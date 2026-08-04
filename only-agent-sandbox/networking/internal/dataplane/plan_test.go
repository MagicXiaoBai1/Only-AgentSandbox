package dataplane

import (
	"bytes"
	"net/netip"
	"strings"
	"testing"

	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/config"
)

const planConfigJSON = `{
  "cniVersion": "1.0.0",
  "name": "k8s-pod-network",
  "type": "oas-vm-net",
  "tapName": "tapH0",
  "tapAddress": "172.16.0.1/30",
  "tapMac": "06:00:ac:10:00:01",
  "tapOwnerUid": 1234,
  "tapOwnerGid": 1234,
  "guestAddress": "172.16.0.2/30",
  "guestMac": "06:00:ac:10:00:02",
  "ingressTCPPorts": [22, 443],
  "controlPlaneCIDRs": ["10.20.0.0/16"],
  "runtimeConfig": {"dns": {"servers": ["8.8.8.8"]}}
}`

func testAttachment(t *testing.T) Attachment {
	t.Helper()
	return Attachment{
		ContainerID: "deadbeef",
		NetNS:       "/var/run/netns/oas-test",
		IfName:      "eth0",
		PodIP:       netip.MustParseAddr("10.10.0.5"),
		MTU:         1440,
	}
}

func TestBuildPlan_OK(t *testing.T) {
	conf, err := config.Parse([]byte(planConfigJSON))
	if err != nil {
		t.Fatalf("parse config: %v", err)
	}

	plan, err := BuildPlan(conf, testAttachment(t))
	if err != nil {
		t.Fatalf("build plan: %v", err)
	}

	if plan.Tap.Name != "tapH0" {
		t.Fatalf("tap name = %q", plan.Tap.Name)
	}
	if plan.Tap.GuestIP.String() != "172.16.0.2" {
		t.Fatalf("guest ip = %s", plan.Tap.GuestIP)
	}
	if plan.DNSServer.String() != "8.8.8.8" {
		t.Fatalf("dns = %s", plan.DNSServer)
	}
	if !strings.HasPrefix(plan.Marker, "oas-vm-net:") {
		t.Fatalf("marker = %q", plan.Marker)
	}
	if plan.NFTCreate == "" {
		t.Fatal("nft script empty")
	}
}

func TestBuildPlan_Rejects(t *testing.T) {
	conf, err := config.Parse([]byte(planConfigJSON))
	if err != nil {
		t.Fatalf("parse config: %v", err)
	}

	cases := []struct {
		name string
		att  Attachment
		want string
	}{
		{"empty container id", Attachment{NetNS: "/x", IfName: "eth0", PodIP: netip.MustParseAddr("10.10.0.5"), MTU: 1440}, "container ID"},
		{"empty netns", Attachment{ContainerID: "x", IfName: "eth0", PodIP: netip.MustParseAddr("10.10.0.5"), MTU: 1440}, "network namespace"},
		{"ifname equals tap", Attachment{ContainerID: "x", NetNS: "/x", IfName: "tapH0", PodIP: netip.MustParseAddr("10.10.0.5"), MTU: 1440}, "different names"},
		{"pod ip in guest subnet", Attachment{ContainerID: "x", NetNS: "/x", IfName: "eth0", PodIP: netip.MustParseAddr("172.16.0.2"), MTU: 1440}, "overlaps fixed guest subnet"},
		{"mtu too small", Attachment{ContainerID: "x", NetNS: "/x", IfName: "eth0", PodIP: netip.MustParseAddr("10.10.0.5"), MTU: 100}, "invalid CNI MTU"},
		{"loopback pod ip", Attachment{ContainerID: "x", NetNS: "/x", IfName: "eth0", PodIP: netip.MustParseAddr("127.0.0.1"), MTU: 1440}, "unicast IPv4"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := BuildPlan(conf, tc.att)
			if err == nil || !strings.Contains(err.Error(), tc.want) {
				t.Fatalf("error = %v, want substring %q", err, tc.want)
			}
		})
	}
}

// TestRenderNFT_Content verifies the generated nftables script carries the
// expected rules and exactly the owned-rule marker count (7 ip + 4 netdev).
func TestRenderNFT_Content(t *testing.T) {
	conf, err := config.Parse([]byte(planConfigJSON))
	if err != nil {
		t.Fatalf("parse config: %v", err)
	}
	plan, err := BuildPlan(conf, testAttachment(t))
	if err != nil {
		t.Fatalf("build plan: %v", err)
	}
	script := plan.NFTCreate

	wantSubstrings := []string{
		`add table ip oas_vm`,
		`add table netdev oas_guard`,
		// inbound to pod IP -> guest IP
		`iifname "eth0" ip daddr 10.10.0.5 dnat to 172.16.0.2`,
		// guest DNS hijack
		`udp dport 53 dnat to 8.8.8.8:53`,
		`tcp dport 53 dnat to 8.8.8.8:53`,
		// guest IP -> pod IP SNAT
		`oifname "eth0" ip saddr 172.16.0.2 snat to 10.10.0.5`,
		// forward: established, guest->eth0, control-plane->guest
		`ct state established,related accept`,
		`iifname "tapH0" oifname "eth0" ip saddr 172.16.0.2 accept`,
		`ip saddr { 10.20.0.0/16 } tcp dport { 22, 443 } accept`,
		// netdev ingress on the bare (unquoted) tap device name
		`hook ingress device tapH0 priority filter`,
		`ether saddr != 06:00:ac:10:00:02 drop`,
		`arp saddr ip != 172.16.0.2 drop`,
	}
	for _, want := range wantSubstrings {
		if !strings.Contains(script, want) {
			t.Errorf("nft script missing %q\n--- script ---\n%s", want, script)
		}
	}

	// The device name must NOT be quoted (nft `device` takes a bare identifier).
	if strings.Contains(script, `device "tapH0"`) {
		t.Errorf("netdev device name must be bare, not quoted\n%s", script)
	}

	// Exactly 11 marker comments: 7 in the ip table, 4 in the netdev table.
	if got := bytes.Count([]byte(script), []byte(plan.Marker)); got != 11 {
		t.Errorf("marker count = %d, want 11\n%s", got, script)
	}
}

func TestAttachmentMarker_Stable(t *testing.T) {
	conf, err := config.Parse([]byte(planConfigJSON))
	if err != nil {
		t.Fatalf("parse config: %v", err)
	}
	att := testAttachment(t)
	p1, _ := BuildPlan(conf, att)
	p2, _ := BuildPlan(conf, att)
	if p1.Marker != p2.Marker {
		t.Fatal("marker not stable for identical attachment")
	}

	att.ContainerID = "different"
	p3, _ := BuildPlan(conf, att)
	if p3.Marker == p1.Marker {
		t.Fatal("marker did not change with container id")
	}
}
