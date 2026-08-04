//go:build linux

package dataplane

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"

	"github.com/vishvananda/netlink"
	"github.com/vishvananda/netns"
)

const (
	oasIPTable    = "oas_vm"
	oasNetdevTable = "oas_guard"
)

// Rule counts owned by a single attachment. The ip table carries 7 marker
// rules (3 prerouting DNAT, 1 postrouting SNAT, 3 forward) and the netdev
// ingress table carries 4 marker rules. Keep in sync with renderNFT.
const (
	ipOwnedRules    = 7
	netdevOwnedRules = 4
)

type LinuxAdapter struct {
	nftPath string
}

func NewLinuxAdapter() *LinuxAdapter {
	return &LinuxAdapter{nftPath: "nft"}
}

// DiscoverAttachment reads the live CNI interface MTU from the sandbox netns
// and confirms the interface carries the Calico-assigned pod IP as a /32.
func (a *LinuxAdapter) DiscoverAttachment(
	netnsPath string,
	ifName string,
	podIP netip.Addr,
) (int, error) {
	var mtu int
	err := withNetLink(netnsPath, func(handle *netlink.Handle) error {
		link, err := handle.LinkByName(ifName)
		if err != nil {
			return fmt.Errorf("find CNI interface %s: %w", ifName, err)
		}
		if link.Attrs().Flags&net.FlagUp == 0 {
			return fmt.Errorf("CNI interface %s is down", ifName)
		}
		if !linkHasIPv4(handle, link, podIP, 32) {
			return fmt.Errorf("CNI interface %s does not have %s/32", ifName, podIP)
		}
		mtu = link.Attrs().MTU
		return nil
	})
	if err != nil {
		return 0, err
	}
	return mtu, nil
}

func (a *LinuxAdapter) InspectAttachment(plan *Plan) (LinkState, error) {
	var state LinkState
	err := withNetLink(plan.Attachment.NetNS, func(handle *netlink.Handle) error {
		cniLink, err := handle.LinkByName(plan.Attachment.IfName)
		if err != nil {
			if isLinkNotFound(err) {
				return nil
			}
			return err
		}
		state.CNIExists = true
		state.CNIMTU = cniLink.Attrs().MTU
		state.HasPodIP = linkHasIPv4(handle, cniLink, plan.Attachment.PodIP, 32)

		if _, err := handle.LinkByName(plan.Tap.Name); err == nil {
			state.TapExists = true
		} else if !isLinkNotFound(err) {
			return err
		}
		return nil
	})
	return state, err
}

func (a *LinuxAdapter) EnsureTap(plan *Plan) error {
	return withNetLink(plan.Attachment.NetNS, func(handle *netlink.Handle) error {
		tap := &netlink.Tuntap{
			LinkAttrs: netlink.LinkAttrs{
				Name: plan.Tap.Name,
				MTU:  plan.Tap.MTU,
			},
			Mode:       netlink.TUNTAP_MODE_TAP,
			Flags:      netlink.TUNTAP_DEFAULTS | netlink.TUNTAP_NO_PI,
			NonPersist: false,
			Owner:      plan.Tap.OwnerUID,
			Group:      plan.Tap.OwnerGID,
		}
		if err := handle.LinkAdd(tap); err != nil {
			return fmt.Errorf("add tap link %s: %w", plan.Tap.Name, err)
		}

		success := false
		defer func() {
			if !success {
				closeTapQueues(tap)
				_ = handle.LinkDel(tap)
			}
		}()

		if err := handle.LinkSetAlias(tap, plan.Marker); err != nil {
			return fmt.Errorf("set tap alias: %w", err)
		}
		mac, err := net.ParseMAC(plan.Tap.MAC)
		if err != nil {
			return fmt.Errorf("parse tap mac %s: %w", plan.Tap.MAC, err)
		}
		if err := handle.LinkSetHardwareAddr(tap, mac); err != nil {
			return fmt.Errorf("set tap mac: %w", err)
		}
		if err := handle.LinkSetMTU(tap, plan.Tap.MTU); err != nil {
			return fmt.Errorf("set tap mtu: %w", err)
		}
		addr, err := netlink.ParseAddr(plan.Tap.Address.String())
		if err != nil {
			return fmt.Errorf("parse tap address %s: %w", plan.Tap.Address, err)
		}
		if err := handle.AddrReplace(tap, addr); err != nil {
			return fmt.Errorf("set tap address: %w", err)
		}
		if err := handle.LinkSetUp(tap); err != nil {
			return fmt.Errorf("bring tap up: %w", err)
		}
		success = true
		return nil
	})
}

func (a *LinuxAdapter) SetForwarding(plan *Plan) error {
	return withNetNS(plan.Attachment.NetNS, func() error {
		return os.WriteFile("/proc/sys/net/ipv4/ip_forward", []byte("1"), 0644)
	})
}

func (a *LinuxAdapter) ApplyNFT(plan *Plan) error {
	return withNetNS(plan.Attachment.NetNS, func() error {
		ipExists, err := a.ownedTableExists("ip", oasIPTable, plan.Marker, ipOwnedRules)
		if err != nil {
			return err
		}
		netdevExists, err := a.ownedTableExists("netdev", oasNetdevTable, plan.Marker, netdevOwnedRules)
		if err != nil {
			return err
		}

		var transaction strings.Builder
		if netdevExists {
			transaction.WriteString("delete table netdev ")
			transaction.WriteString(oasNetdevTable)
			transaction.WriteByte('\n')
		}
		if ipExists {
			transaction.WriteString("delete table ip ")
			transaction.WriteString(oasIPTable)
			transaction.WriteByte('\n')
		}
		transaction.WriteString(plan.NFTCreate)
		return a.runNFT(transaction.String())
	})
}

func (a *LinuxAdapter) Check(plan *Plan) error {
	return withNetLink(plan.Attachment.NetNS, func(handle *netlink.Handle) error {
		cniLink, err := handle.LinkByName(plan.Attachment.IfName)
		if err != nil {
			return fmt.Errorf("CNI interface: %w", err)
		}
		if cniLink.Attrs().Flags&net.FlagUp == 0 ||
			cniLink.Attrs().MTU != plan.Attachment.MTU ||
			!linkHasIPv4(handle, cniLink, plan.Attachment.PodIP, 32) {
			return fmt.Errorf("CNI interface state does not match attachment")
		}

		tapLink, err := handle.LinkByName(plan.Tap.Name)
		if err != nil {
			return fmt.Errorf("tap: %w", err)
		}
		tap, ok := tapLink.(*netlink.Tuntap)
		if !ok {
			return fmt.Errorf("%s is not a tap device", plan.Tap.Name)
		}
		if tap.Mode != netlink.TUNTAP_MODE_TAP || tap.NonPersist {
			return fmt.Errorf("%s is not a persistent tap", plan.Tap.Name)
		}
		if tap.Attrs().Alias != plan.Marker ||
			tap.Attrs().Flags&net.FlagUp == 0 ||
			tap.Attrs().MTU != plan.Tap.MTU ||
			tap.Attrs().HardwareAddr.String() != plan.Tap.MAC ||
			tap.Owner != plan.Tap.OwnerUID ||
			tap.Group != plan.Tap.OwnerGID ||
			!linkHasIPv4(handle, tap, plan.Tap.Address.Addr(), plan.Tap.Address.Bits()) {
			return fmt.Errorf("tap state does not match desired configuration")
		}

		forwarding, err := os.ReadFile("/proc/sys/net/ipv4/ip_forward")
		if err != nil || strings.TrimSpace(string(forwarding)) != "1" {
			return fmt.Errorf("IPv4 forwarding is not enabled")
		}
		if err := a.checkOwnedTable("ip", oasIPTable, plan.Marker, ipOwnedRules); err != nil {
			return err
		}
		if err := a.checkOwnedTable("netdev", oasNetdevTable, plan.Marker, netdevOwnedRules); err != nil {
			return err
		}
		if err := checkRoute(handle, plan.Tap.GuestIP, tap.Attrs().Index); err != nil {
			return fmt.Errorf("guest route: %w", err)
		}
		if err := checkRoute(handle, netip.MustParseAddr("1.1.1.1"), cniLink.Attrs().Index); err != nil {
			return fmt.Errorf("egress route: %w", err)
		}
		return nil
	})
}

func (a *LinuxAdapter) DeleteNFT(plan *Plan) error {
	if plan == nil || plan.Attachment.NetNS == "" {
		return nil
	}
	return withNetNS(plan.Attachment.NetNS, func() error {
		return errors.Join(
			a.deleteOwnedTable("netdev", oasNetdevTable, plan.Marker),
			a.deleteOwnedTable("ip", oasIPTable, plan.Marker),
		)
	})
}

func (a *LinuxAdapter) DeleteTap(plan *Plan) error {
	if plan == nil || plan.Attachment.NetNS == "" {
		return nil
	}
	return withNetLink(plan.Attachment.NetNS, func(handle *netlink.Handle) error {
		link, err := handle.LinkByName(plan.Tap.Name)
		if isLinkNotFound(err) {
			return nil
		}
		if err != nil {
			return err
		}
		if link.Attrs().Alias != plan.Marker {
			return fmt.Errorf("refusing to delete unowned link %s", plan.Tap.Name)
		}
		return handle.LinkDel(link)
	})
}

func (a *LinuxAdapter) checkOwnedTable(family, table, marker string, ruleCount int) error {
	output, err := a.nftOutput("list", "table", family, table)
	if err != nil {
		return fmt.Errorf("list nft table %s %s: %w", family, table, err)
	}
	if got := bytes.Count(output, []byte(marker)); got != ruleCount {
		return fmt.Errorf("nft table %s %s has %d owned rules, want %d",
			family, table, got, ruleCount)
	}
	return nil
}

func (a *LinuxAdapter) deleteOwnedTable(family, table, marker string) error {
	ruleCount := ipOwnedRules
	if family == "netdev" {
		ruleCount = netdevOwnedRules
	}
	exists, err := a.ownedTableExists(family, table, marker, ruleCount)
	if err != nil || !exists {
		return err
	}
	_, err = a.nftOutput("delete", "table", family, table)
	return err
}

func (a *LinuxAdapter) ownedTableExists(
	family, table, marker string,
	ruleCount int,
) (bool, error) {
	output, err := a.nftOutput("list", "table", family, table)
	if err != nil {
		if nftObjectMissing(output) {
			return false, nil
		}
		return false, err
	}
	if got := bytes.Count(output, []byte(marker)); got != ruleCount {
		return false, fmt.Errorf(
			"refusing to replace nft table %s %s with %d owned rules, want %d",
			family, table, got, ruleCount,
		)
	}
	return true, nil
}

func (a *LinuxAdapter) runNFT(script string) error {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, a.nftPath, "-f", "-")
	cmd.Stdin = strings.NewReader(script)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("nft transaction: %w: %s", err, strings.TrimSpace(string(output)))
	}
	return nil
}

func (a *LinuxAdapter) nftOutput(args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, a.nftPath, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return output, fmt.Errorf("%w: %s", err, strings.TrimSpace(string(output)))
	}
	return output, nil
}

// withNetNS enters the network namespace at path, runs fn, and always restores
// the original namespace. The target namespace must differ from the current one
// so a misconfigured path cannot silently mutate the plugin's own namespace.
func withNetNS(path string, fn func() error) error {
	ns, err := netns.GetFromPath(path)
	if err != nil {
		return fmt.Errorf("open target network namespace: %w", err)
	}
	defer ns.Close()

	runtime.LockOSThread()
	unlock := true
	entered := false
	var runErr error
	current := netns.None()
	defer func() {
		if recovered := recover(); recovered != nil {
			runErr = errors.Join(runErr, fmt.Errorf("panic in target network namespace: %v", recovered))
		}
		if entered {
			if err := netns.Set(current); err != nil {
				runErr = errors.Join(runErr, fmt.Errorf("restore network namespace: %w", err))
			}
			unlock = false
		}
		if unlock {
			runtime.UnlockOSThread()
		}
	}()

	current, err = netns.Get()
	if err != nil {
		return fmt.Errorf("open current network namespace: %w", err)
	}
	if current.Equal(ns) {
		return fmt.Errorf("target network namespace must differ from plugin namespace")
	}
	if err := netns.Set(ns); err != nil {
		return fmt.Errorf("enter target network namespace: %w", err)
	}
	entered = true
	runErr = fn()
	return runErr
}

func withNetLink(path string, fn func(*netlink.Handle) error) error {
	return withNetNS(path, func() error {
		handle, err := netlink.NewHandle()
		if err != nil {
			return fmt.Errorf("open netlink handle in target namespace: %w", err)
		}
		defer handle.Close()
		return fn(handle)
	})
}

func linkHasIPv4(
	handle *netlink.Handle,
	link netlink.Link,
	expected netip.Addr,
	prefixBits int,
) bool {
	addresses, err := handle.AddrList(link, netlink.FAMILY_V4)
	if err != nil {
		return false
	}
	for _, address := range addresses {
		if address.IPNet.IP.IsLoopback() {
			continue
		}
		ip, ok := netip.AddrFromSlice(address.IP.To4())
		if !ok || ip != expected {
			continue
		}
		ones, bits := address.Mask.Size()
		if bits == 32 && ones == prefixBits {
			return true
		}
	}
	return false
}

func checkRoute(
	handle *netlink.Handle,
	destination netip.Addr,
	expectedIndex int,
) error {
	routes, err := handle.RouteGet(net.IP(destination.AsSlice()))
	if err != nil {
		return err
	}
	for _, route := range routes {
		if route.LinkIndex == expectedIndex {
			return nil
		}
	}
	return fmt.Errorf("no route to %s via interface index %d", destination, expectedIndex)
}

func closeTapQueues(tap *netlink.Tuntap) {
	for _, file := range tap.Fds {
		_ = file.Close()
	}
	tap.Fds = nil
}

func isLinkNotFound(err error) bool {
	var notFound netlink.LinkNotFoundError
	return errors.As(err, &notFound)
}

func nftObjectMissing(output []byte) bool {
	return strings.Contains(string(output), "No such file or directory")
}
