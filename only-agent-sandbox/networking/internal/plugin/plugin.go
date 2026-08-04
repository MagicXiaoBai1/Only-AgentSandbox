package plugin

import (
	"errors"
	"fmt"

	"github.com/containernetworking/cni/pkg/skel"
	types100 "github.com/containernetworking/cni/pkg/types/100"

	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/config"
	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/dataplane"
	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/result"
)

// Runner wires the CNI lifecycle to the dataplane manager. It is safe to reuse
// across ADD/CHECK/DEL for the same plugin process.
type Runner struct {
	adapter dataplane.Adapter
}

// New builds a Runner backed by the given dataplane adapter.
func New(adapter dataplane.Adapter) *Runner {
	return &Runner{adapter: adapter}
}

// Add runs the OAS dataplane setup and returns the CNI result with the TAP
// interface appended to Calico's prevResult.
func (r *Runner) Add(args *skel.CmdArgs) (*types100.Result, error) {
	if err := validateArgs(args, true); err != nil {
		return nil, err
	}

	conf, parsed, attachment, err := r.prepare(args)
	if err != nil {
		return nil, err
	}

	plan, err := dataplane.BuildPlan(conf, attachment)
	if err != nil {
		return nil, err
	}

	if err := dataplane.NewManager(r.adapter).Add(plan); err != nil {
		return nil, err
	}

	return result.AppendTap(parsed, conf.TapName, conf.TapMAC.String(), args.Netns, attachment.MTU)
}

// Check verifies that the dataplane still matches the desired state.
func (r *Runner) Check(args *skel.CmdArgs) error {
	if err := validateArgs(args, true); err != nil {
		return err
	}

	conf, _, attachment, err := r.prepare(args)
	if err != nil {
		return err
	}

	plan, err := dataplane.BuildPlan(conf, attachment)
	if err != nil {
		return err
	}

	return dataplane.NewManager(r.adapter).Check(plan)
}

// Delete removes only the resources owned by this attachment. Errors are
// best-effort: the netns is about to disappear, so the caller swallows them.
func (r *Runner) Delete(args *skel.CmdArgs) error {
	if args == nil {
		return nil
	}
	if args.Netns == "" {
		return nil
	}
	if args.ContainerID == "" || args.IfName == "" {
		return fmt.Errorf("CNI_CONTAINERID and CNI_IFNAME are required")
	}
	if r == nil || r.adapter == nil {
		return fmt.Errorf("dataplane adapter is required")
	}

	conf, err := config.Parse(args.StdinData)
	if err != nil {
		return err
	}

	plan, err := dataplane.BuildCleanupPlan(conf, dataplane.Attachment{
		ContainerID: args.ContainerID,
		NetNS:       args.Netns,
		IfName:      args.IfName,
	})
	if err != nil {
		return err
	}

	return dataplane.NewManager(r.adapter).Delete(plan)
}

// prepare parses the configuration and prevResult and discovers the live CNI
// attachment (MTU) from the sandbox netns.
func (r *Runner) prepare(
	args *skel.CmdArgs,
) (*config.Config, *result.Parsed, dataplane.Attachment, error) {
	conf, err := config.Parse(args.StdinData)
	if err != nil {
		return nil, nil, dataplane.Attachment{}, err
	}

	parsed, err := result.Parse(conf.RawPrevResult, args.IfName, args.Netns)
	if err != nil {
		return nil, nil, dataplane.Attachment{}, err
	}

	mtu, err := r.adapter.DiscoverAttachment(args.Netns, args.IfName, parsed.PodIP)
	if err != nil {
		return nil, nil, dataplane.Attachment{}, fmt.Errorf("validate CNI attachment: %w", err)
	}

	attachment := dataplane.Attachment{
		ContainerID: args.ContainerID,
		NetNS:       args.Netns,
		IfName:      args.IfName,
		PodIP:       parsed.PodIP,
		MTU:         mtu,
	}

	return conf, parsed, attachment, nil
}

func validateArgs(args *skel.CmdArgs, requireNetNS bool) error {
	if args == nil {
		return errors.New("CNI arguments are required")
	}

	var missing []string
	if args.ContainerID == "" {
		missing = append(missing, "CNI_CONTAINERID")
	}
	if args.IfName == "" {
		missing = append(missing, "CNI_IFNAME")
	}
	if requireNetNS && args.Netns == "" {
		missing = append(missing, "CNI_NETNS")
	}
	if len(args.StdinData) == 0 {
		missing = append(missing, "stdin network configuration")
	}

	if len(missing) != 0 {
		return fmt.Errorf("missing required CNI inputs: %v", missing)
	}

	return nil
}
