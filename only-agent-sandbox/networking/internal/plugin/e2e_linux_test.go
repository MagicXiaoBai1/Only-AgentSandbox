//go:build linux

package plugin

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/containernetworking/cni/pkg/skel"
	types100 "github.com/containernetworking/cni/pkg/types/100"
	"github.com/vishvananda/netlink"
	"github.com/vishvananda/netns"

	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/dataplane"
)

// TestE2E_AddCheckDelete drives the full CNI lifecycle (ADD/CHECK/DEL) of the
// oas-vm-net plugin against a real, isolated network namespace with a veth
// acting as the Calico eth0, a persistent TAP, nftables rules and IPv4
// forwarding. It requires root, Linux and the nft binary.
//
// Run with: sudo -E go test -run TestE2E ./internal/plugin
func TestE2E_AddCheckDelete(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("e2e test requires root (CAP_NET_ADMIN to create netns/tap/nft)")
	}
	if _, err := exec.LookPath("nft"); err != nil {
		t.Skip("nft binary not available")
	}

	env := newSandboxEnv(t)
	t.Cleanup(env.cleanup)

	// Bring the fake Calico eth0 up with the pod /32 and a default route so
	// the plugin's egress-route check (1.1.1.1 via eth0) resolves.
	const podIP = "10.10.0.5"
	env.installCalicoEth0(t, podIP)

	// Build the full CNI configuration, embedding a prevResult that describes
	// the Calico eth0 we just installed in the sandbox netns.
	stdin := buildCNIConfig(t, env.netnsPath, podIP)

	args := &skel.CmdArgs{
		ContainerID: "oas-e2e-deadbeef",
		Netns:       env.netnsPath,
		IfName:      "eth0",
		StdinData:   stdin,
	}

	runner := New(dataplane.NewLinuxAdapter())

	// --- ADD ---
	result, err := runner.Add(args)
	if err != nil {
		t.Fatalf("ADD failed: %v", err)
	}
	if result == nil {
		t.Fatal("ADD returned nil result")
	}
	if !resultHasTap(result, "tapH0", env.netnsPath) {
		t.Fatalf("ADD result does not contain tapH0 in sandbox %s\n%s",
			env.netnsPath, mustJSON(result))
	}

	// Verify the dataplane is really there in the live netns.
	env.requireTapInstalled(t)
	env.requireNFTInstalled(t)

	// --- CHECK ---
	if err := runner.Check(args); err != nil {
		t.Fatalf("CHECK after ADD failed: %v", err)
	}

	// --- DEL ---
	if err := runner.Delete(args); err != nil {
		t.Fatalf("DEL failed: %v", err)
	}
	env.requireTapGone(t)

	// A second DEL must be idempotent (resources already gone with the marker).
	if err := runner.Delete(args); err != nil {
		t.Fatalf("second (idempotent) DEL failed: %v", err)
	}
}

// TestE2E_AddRollback verifies that a failed ADD (tap already exists from a
// prior attachment) does not mutate or delete the pre-existing tap.
func TestE2E_AddRefusesExistingTap(t *testing.T) {
	if os.Geteuid() != 0 {
		t.Skip("e2e test requires root")
	}
	if _, err := exec.LookPath("nft"); err != nil {
		t.Skip("nft binary not available")
	}

	env := newSandboxEnv(t)
	t.Cleanup(env.cleanup)

	const podIP = "10.10.0.6"
	env.installCalicoEth0(t, podIP)

	// Pre-create the tap so InspectAttachment sees TapExists.
	env.precreateTap(t)

	runner := New(dataplane.NewLinuxAdapter())
	args := &skel.CmdArgs{
		ContainerID: "oas-e2e-rollback",
		Netns:       env.netnsPath,
		IfName:      "eth0",
		StdinData:   buildCNIConfig(t, env.netnsPath, podIP),
	}

	if _, err := runner.Add(args); err == nil {
		t.Fatal("ADD with pre-existing tap should fail, got nil error")
	}

	// The pre-created tap must still be present and untouched.
	env.requireTapPresent(t, "precreatedTap")
}

// ---- sandbox environment helpers ----

type sandboxEnv struct {
	netnsName string
	netnsPath string
	nsHandle  netns.NsHandle
	hostVeth  string
}

func newSandboxEnv(t *testing.T) *sandboxEnv {
	t.Helper()
	name := fmt.Sprintf("oas-e2e-%d", os.Getpid())
	ns, err := netns.NewNamed(name)
	if err != nil {
		t.Fatalf("create named netns: %v", err)
	}
	return &sandboxEnv{
		netnsName: name,
		netnsPath: filepath.Join("/var/run/netns", name),
		nsHandle:  ns,
		hostVeth:  "oashost" + name,
	}
}

func (e *sandboxEnv) cleanup() {
	// Remove the host-side veth if it still exists.
	if host, err := netlink.NewHandle(); err == nil {
		if link, err := host.LinkByName(e.hostVeth); err == nil {
			_ = host.LinkDel(link)
		}
		host.Close()
	}
	if e.nsHandle.IsOpen() {
		_ = e.nsHandle.Close()
	}
	_ = netns.DeleteNamed(e.netnsName)
}

// installCalicoEth0 creates a veth pair, moves the peer into the sandbox netns
// as eth0, assigns podIP/32, brings it and lo up, and installs a default route
// via a dummy link-local gateway so the plugin's egress check resolves.
func (e *sandboxEnv) installCalicoEth0(t *testing.T, podIP string) {
	t.Helper()

	host, err := netlink.NewHandle()
	if err != nil {
		t.Fatalf("open host netlink handle: %v", err)
	}
	defer host.Close()

	peerName := "oaspeer" + e.netnsName
	veth := &netlink.Veth{
		LinkAttrs: netlink.LinkAttrs{Name: e.hostVeth, MTU: 1440},
		PeerName:  peerName,
	}
	if err := host.LinkAdd(veth); err != nil {
		t.Fatalf("add veth pair: %v", err)
	}

	peer, err := host.LinkByName(peerName)
	if err != nil {
		t.Fatalf("find veth peer: %v", err)
	}
	if err := host.LinkSetNsFd(peer, int(e.nsHandle)); err != nil {
		t.Fatalf("move peer into sandbox netns: %v", err)
	}

	pod, err := netlink.NewHandleAt(e.nsHandle)
	if err != nil {
		t.Fatalf("open pod netlink handle: %v", err)
	}
	defer pod.Close()

	eth0, err := pod.LinkByName(peerName)
	if err != nil {
		t.Fatalf("find peer in sandbox: %v", err)
	}
	if err := pod.LinkSetName(eth0, "eth0"); err != nil {
		t.Fatalf("rename peer to eth0: %v", err)
	}

	addr, err := netlink.ParseAddr(podIP + "/32")
	if err != nil {
		t.Fatalf("parse pod addr: %v", err)
	}
	if err := pod.AddrAdd(eth0, addr); err != nil {
		t.Fatalf("assign pod addr: %v", err)
	}
	if err := pod.LinkSetUp(eth0); err != nil {
		t.Fatalf("bring eth0 up: %v", err)
	}

	if lo, err := pod.LinkByName("lo"); err == nil {
		_ = pod.LinkSetUp(lo)
	}

	// Dummy gateway reachable out eth0 + default route via it.
	gw := net.ParseIP("169.254.1.1")
	eth0Idx := eth0.Attrs().Index
	if err := pod.RouteAdd(&netlink.Route{
		Dst:       &net.IPNet{IP: gw, Mask: net.CIDRMask(32, 32)},
		LinkIndex: eth0Idx,
	}); err != nil {
		t.Fatalf("add gateway host route: %v", err)
	}
	if err := pod.RouteAdd(&netlink.Route{
		Dst:       nil,
		Gw:        gw,
		LinkIndex: eth0Idx,
	}); err != nil {
		t.Fatalf("add default route: %v", err)
	}

	// Bring the host end up so the link has a carrier.
	if hostVeth, err := host.LinkByName(e.hostVeth); err == nil {
		_ = host.LinkSetUp(hostVeth)
	}
}

func (e *sandboxEnv) precreateTap(t *testing.T) {
	t.Helper()
	pod, err := netlink.NewHandleAt(e.nsHandle)
	if err != nil {
		t.Fatalf("open pod netlink handle: %v", err)
	}
	defer pod.Close()
	tap := &netlink.Tuntap{
		LinkAttrs:   netlink.LinkAttrs{Name: "tapH0"},
		Mode:        netlink.TUNTAP_MODE_TAP,
		Flags:       netlink.TUNTAP_DEFAULTS | netlink.TUNTAP_NO_PI,
		NonPersist:  false,
	}
	if err := pod.LinkAdd(tap); err != nil {
		t.Fatalf("precreate tap: %v", err)
	}
	_ = pod.LinkSetUp(tap)
}

func (e *sandboxEnv) requireTapInstalled(t *testing.T) {
	t.Helper()
	pod, err := netlink.NewHandleAt(e.nsHandle)
	if err != nil {
		t.Fatalf("open pod handle: %v", err)
	}
	defer pod.Close()
	link, err := pod.LinkByName("tapH0")
	if err != nil {
		t.Fatalf("tapH0 not present after ADD: %v", err)
	}
	tap, ok := link.(*netlink.Tuntap)
	if !ok {
		t.Fatalf("tapH0 is not a TAP device: %T", link)
	}
	if tap.Mode != netlink.TUNTAP_MODE_TAP || tap.NonPersist {
		t.Errorf("tapH0 not a persistent tap: mode=%v nonPersist=%v", tap.Mode, tap.NonPersist)
	}
}

func (e *sandboxEnv) requireNFTInstalled(t *testing.T) {
	t.Helper()
	// Listing must succeed and the tables must exist.
	for _, tc := range []struct{ family, table string }{
		{"ip", "oas_vm"},
		{"netdev", "oas_guard"},
	} {
		if err := exec.Command("nft", "list", "table", tc.family, tc.table).Run(); err != nil {
			t.Errorf("nft table %s %s missing after ADD: %v", tc.family, tc.table, err)
		}
	}
}

func (e *sandboxEnv) requireTapGone(t *testing.T) {
	t.Helper()
	pod, err := netlink.NewHandleAt(e.nsHandle)
	if err != nil {
		t.Fatalf("open pod handle: %v", err)
	}
	defer pod.Close()
	if _, err := pod.LinkByName("tapH0"); err == nil {
		t.Fatal("tapH0 still present after DEL")
	}
}

func (e *sandboxEnv) requireTapPresent(t *testing.T, why string) {
	t.Helper()
	pod, err := netlink.NewHandleAt(e.nsHandle)
	if err != nil {
		t.Fatalf("open pod handle: %v", err)
	}
	defer pod.Close()
	if _, err := pod.LinkByName("tapH0"); err != nil {
		t.Fatalf("%s: tapH0 should still exist but is gone: %v", why, err)
	}
}

// ---- CNI config / result helpers ----

func buildCNIConfig(t *testing.T, netnsPath, podIP string) []byte {
	t.Helper()
	cfg := map[string]any{
		"cniVersion": "1.0.0",
		"name":       "k8s-pod-network",
		"type":       "oas-vm-net",
		"tapName":    "tapH0",
		"tapAddress": "172.16.0.1/30",
		"tapMac":     "06:00:ac:10:00:01",
		"tapOwnerUid": 1234,
		"tapOwnerGid": 1234,
		"guestAddress": "172.16.0.2/30",
		"guestMac":     "06:00:ac:10:00:02",
		"ingressTCPPorts":   []int{22},
		"controlPlaneCIDRs": []string{"10.20.0.0/16"},
		"runtimeConfig": map[string]any{
			"dns": map[string]any{"servers": []string{"8.8.8.8"}},
		},
		"prevResult": map[string]any{
			"cniVersion": "1.0.0",
			"interfaces": []map[string]any{
				{"name": "eth0", "sandbox": netnsPath, "mac": "06:00:de:ad:be:ef"},
			},
			"ips": []map[string]any{
				{"interface": 0, "address": podIP + "/32"},
			},
			"routes": []map[string]any{},
		},
	}
	data, err := json.Marshal(cfg)
	if err != nil {
		t.Fatalf("marshal config: %v", err)
	}
	return data
}

func resultHasTap(result *types100.Result, name, sandbox string) bool {
	for _, iface := range result.Interfaces {
		if iface != nil && iface.Name == name && iface.Sandbox == sandbox {
			return true
		}
	}
	return false
}

func mustJSON(v any) string {
	data, _ := json.MarshalIndent(v, "", "  ")
	return string(data)
}
