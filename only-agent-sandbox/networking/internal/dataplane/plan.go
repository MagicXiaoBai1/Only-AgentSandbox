package dataplane

import (
	"crypto/sha256"
	"fmt"
	"net/netip"
	"regexp"
	"strconv"
	"strings"

	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/config"
)

var interfaceNamePattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,14}$`)

type Attachment struct {
	ContainerID string
	NetNS       string
	IfName      string
	PodIP       netip.Addr
	MTU         int
}

type TapPlan struct {
	Name      string
	Address   netip.Prefix
	MAC       string
	OwnerUID  uint32
	OwnerGID  uint32
	MTU       int
	GuestIP   netip.Addr
	GuestMAC  string
}

type Plan struct {
	Attachment Attachment
	Tap        TapPlan
	DNSServer  netip.Addr
	Marker     string
	NFTCreate  string
	NFTScript  string
}

func BuildPlan(conf *config.Config, attachment Attachment) (*Plan, error) {
	if conf == nil {
		return nil, fmt.Errorf("network configuration is required")
	}
	if attachment.ContainerID == "" {
		return nil, fmt.Errorf("container ID is required")
	}
	if attachment.NetNS == "" {
		return nil, fmt.Errorf("network namespace path is required")
	}
	if !interfaceNamePattern.MatchString(attachment.IfName) {
		return nil, fmt.Errorf("invalid CNI interface name %q", attachment.IfName)
	}
	if attachment.IfName == conf.TapName {
		return nil, fmt.Errorf("CNI interface and tap must have different names")
	}
	if !attachment.PodIP.Is4() || !attachment.PodIP.IsValid() ||
		attachment.PodIP.IsUnspecified() || attachment.PodIP.IsLoopback() ||
		attachment.PodIP.IsMulticast() {
		return nil, fmt.Errorf("pod IP must be a unicast IPv4 address")
	}
	if conf.TapAddress.Masked().Contains(attachment.PodIP) {
		return nil, fmt.Errorf("pod IP overlaps fixed guest subnet")
	}
	if attachment.MTU < 576 || attachment.MTU > 65535 {
		return nil, fmt.Errorf("invalid CNI MTU %d", attachment.MTU)
	}
	if len(conf.DNS.Servers) == 0 {
		return nil, fmt.Errorf("at least one IPv4 DNS server is required")
	}

	marker := attachmentMarker(conf, attachment)

	plan := &Plan{
		Attachment: attachment,
		Tap: TapPlan{
			Name:     conf.TapName,
			Address:  conf.TapAddress,
			MAC:      conf.TapMAC.String(),
			OwnerUID: conf.TapOwnerUID,
			OwnerGID: conf.TapOwnerGID,
			MTU:      attachment.MTU,
			GuestIP:  conf.GuestAddress.Addr(),
			GuestMAC: conf.GuestMAC.String(),
		},
		DNSServer: conf.DNS.Servers[0],
		Marker:    marker,
	}

	plan.NFTCreate = renderNFT(conf, plan)
	plan.NFTScript = plan.NFTCreate
	return plan, nil
}

func BuildCleanupPlan(conf *config.Config, attachment Attachment) (*Plan, error) {
	if conf == nil {
		return nil, fmt.Errorf("network configuration is required")
	}
	if attachment.ContainerID == "" || attachment.NetNS == "" {
		return nil, fmt.Errorf("container ID and network namespace are required")
	}
	if !interfaceNamePattern.MatchString(attachment.IfName) {
		return nil, fmt.Errorf("invalid CNI interface name %q", attachment.IfName)
	}
	return &Plan{
		Attachment: attachment,
		Tap: TapPlan{
			Name: conf.TapName,
		},
		Marker: attachmentMarker(conf, attachment),
	}, nil
}

func attachmentMarker(conf *config.Config, attachment Attachment) string {
	markerBytes := sha256.Sum256([]byte(strings.Join([]string{
		conf.Name,
		attachment.ContainerID,
		attachment.NetNS,
		attachment.IfName,
		conf.TapName,
	}, "\x00")))
	return fmt.Sprintf("oas-vm-net:%x", markerBytes[:8])
}

func renderNFT(conf *config.Config, plan *Plan) string {
	controlCIDRs := make([]string, 0, len(conf.ControlPlaneCIDRs))
	for _, prefix := range conf.ControlPlaneCIDRs {
		controlCIDRs = append(controlCIDRs, prefix.String())
	}
	ports := make([]string, 0, len(conf.IngressTCPPorts))
	for _, port := range conf.IngressTCPPorts {
		ports = append(ports, strconv.Itoa(int(port)))
	}

	return fmt.Sprintf(`add table ip oas_vm
flush table ip oas_vm
add chain ip oas_vm prerouting { type nat hook prerouting priority dstnat; policy accept; }
add chain ip oas_vm postrouting { type nat hook postrouting priority srcnat; policy accept; }
add chain ip oas_vm forward { type filter hook forward priority filter; policy drop; }
add rule ip oas_vm prerouting iifname %s ip daddr %s dnat to %s comment %s
add rule ip oas_vm prerouting iifname %s ip daddr %s udp dport 53 dnat to %s:53 comment %s
add rule ip oas_vm prerouting iifname %s ip daddr %s tcp dport 53 dnat to %s:53 comment %s
add rule ip oas_vm postrouting oifname %s ip saddr %s snat to %s comment %s
add rule ip oas_vm forward ct state established,related accept comment %s
add rule ip oas_vm forward iifname %s oifname %s ip saddr %s accept comment %s
add rule ip oas_vm forward iifname %s oifname %s ip daddr %s ip saddr { %s } tcp dport { %s } accept comment %s
add table netdev oas_guard
flush table netdev oas_guard
add chain netdev oas_guard tap_ingress { type filter hook ingress device %s priority filter; policy drop; }
add rule netdev oas_guard tap_ingress ether saddr != %s drop comment %s
add rule netdev oas_guard tap_ingress ether type arp arp saddr ip != %s drop comment %s
add rule netdev oas_guard tap_ingress ether type arp arp saddr ether != %s drop comment %s
add rule netdev oas_guard tap_ingress ether type { ip, arp } accept comment %s
`,
		// prerouting: inbound to pod IP -> guest IP (iif = CNI eth0)
		quote(plan.Attachment.IfName), plan.Attachment.PodIP, plan.Tap.GuestIP, quote(plan.Marker),
		// prerouting: guest DNS (udp/53) -> configured resolver (iif = tap)
		quote(plan.Tap.Name), plan.Tap.Address.Addr(), plan.DNSServer, quote(plan.Marker),
		// prerouting: guest DNS (tcp/53) -> configured resolver (iif = tap)
		quote(plan.Tap.Name), plan.Tap.Address.Addr(), plan.DNSServer, quote(plan.Marker),
		// postrouting: guest IP -> pod IP (oif = CNI eth0)
		quote(plan.Attachment.IfName), plan.Tap.GuestIP, plan.Attachment.PodIP, quote(plan.Marker),
		// forward: established/related return traffic
		quote(plan.Marker),
		// forward: guest -> CNI eth0
		quote(plan.Tap.Name), quote(plan.Attachment.IfName), plan.Tap.GuestIP, quote(plan.Marker),
		// forward: control plane -> guest, only on ingress TCP ports
		quote(plan.Attachment.IfName), quote(plan.Tap.Name), plan.Tap.GuestIP,
		strings.Join(controlCIDRs, ", "), strings.Join(ports, ", "), quote(plan.Marker),
		// netdev ingress hook on the tap device (device name is bare, not quoted)
		plan.Tap.Name,
		// drop frames whose ethernet source is not the guest MAC
		plan.Tap.GuestMAC, quote(plan.Marker),
		// drop ARP whose sender IP is not the guest IP
		plan.Tap.GuestIP, quote(plan.Marker),
		// drop ARP whose sender MAC is not the guest MAC
		plan.Tap.GuestMAC, quote(plan.Marker),
		// allow IP and ARP from the guest
		quote(plan.Marker),
	)
}

func quote(value string) string {
	return strconv.Quote(value)
}