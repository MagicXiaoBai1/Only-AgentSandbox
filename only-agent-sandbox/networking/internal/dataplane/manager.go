package dataplane

import (
	"errors"
	"fmt"
	"net/netip"
)

type LinkState struct {
	CNIExists bool
	CNIMTU    int
	HasPodIP  bool
	TapExists bool
}

type Adapter interface {
	// DiscoverAttachment reads the MTU of the live CNI interface in the
	// sandbox netns and confirms it carries the expected pod IP.
	DiscoverAttachment(netnsPath, ifName string, podIP netip.Addr) (int, error)
	InspectAttachment(*Plan) (LinkState, error)
	EnsureTap(*Plan) error
	SetForwarding(*Plan) error
	ApplyNFT(*Plan) error
	Check(*Plan) error
	DeleteNFT(*Plan) error
	DeleteTap(*Plan) error
}

type Manager struct {
	adapter Adapter
}

func NewManager(adapter Adapter) *Manager {
	return &Manager{adapter: adapter}
}

func (m *Manager) Add(plan *Plan) (retErr error) {
	if m == nil || m.adapter == nil {
		return fmt.Errorf("dataplane adapter is required")
	}

	state, err := m.adapter.InspectAttachment(plan)
	if err != nil {
		return fmt.Errorf("inspect CNI attachment: %w", err)
	}

	if !state.CNIExists || !state.HasPodIP {
		return fmt.Errorf("CNI interface is missing expected pod IP")
	}

	if state.CNIMTU != plan.Attachment.MTU {
		return fmt.Errorf("CNI interface MTU changed from %d to %d", plan.Attachment.MTU, state.CNIMTU)
	}

	if state.TapExists {
		return fmt.Errorf("refusing to replace existing tap %s", plan.Tap.Name)
	}

	tapCreated := false
	defer func() {
		// Roll back only on failure; on success the dataplane stays in place.
		if retErr == nil {
			return
		}
		var cleanupErr error
		cleanupErr = errors.Join(cleanupErr, m.adapter.DeleteNFT(plan))
		if tapCreated {
			cleanupErr = errors.Join(cleanupErr, m.adapter.DeleteTap(plan))
		}
		if cleanupErr != nil {
			retErr = errors.Join(retErr, fmt.Errorf("rollback dataplane: %w", cleanupErr))
		}
	}()

	if err := m.adapter.EnsureTap(plan); err != nil {
		return fmt.Errorf("create tap: %w", err)
	}
	tapCreated = true

	if err := m.adapter.SetForwarding(plan); err != nil {
		return fmt.Errorf("enable forwarding: %w", err)
	}

	if err := m.adapter.ApplyNFT(plan); err != nil {
		return fmt.Errorf("apply nftables rules: %w", err)
	}

	if err := m.adapter.Check(plan); err != nil {
		return fmt.Errorf("verify dataplane: %w", err)
	}

	return nil
}

func (m *Manager) Check(plan *Plan) error {
	if m == nil || m.adapter == nil {
		return fmt.Errorf("dataplane adapter is required")
	}

	if err := m.adapter.Check(plan); err != nil {
		return fmt.Errorf("dataplane check failed: %w", err)
	}

	return nil
}

func (m *Manager) Delete(plan *Plan) error {
	if m == nil || m.adapter == nil {
		return nil
	}
	return errors.Join(
		m.adapter.DeleteNFT(plan),
		m.adapter.DeleteTap(plan),
	)
}