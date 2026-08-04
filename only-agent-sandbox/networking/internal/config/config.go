package config

import (
	"encoding/json"
	"fmt"
	"net"
	"net/netip"
	"regexp"
	"slices"
)

var interfaceNamePattern = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,14}$`)

type Config struct {
	CNIVersion        string
	Name              string
	Type              string
	TapName           string
	TapAddress        netip.Prefix
	TapMAC            net.HardwareAddr
	TapOwnerUID       uint32
	TapOwnerGID       uint32
	GuestAddress      netip.Prefix
	GuestMAC          net.HardwareAddr
	IngressTCPPorts   []uint16
	ControlPlaneCIDRs []netip.Prefix
	DNS               DNS
	RawPrevResult     map[string]any
}

type DNS struct {
	Servers []netip.Addr
}

type rawConfig struct {
	CNIVersion        string            `json:"cniVersion"`
	Name              string            `json:"name"`
	Type              string            `json:"type"`
	TapName           string            `json:"tapName"`
	TapAddress        string            `json:"tapAddress"`
	TapMAC            string            `json:"tapMac"`
	TapOwnerUID       *uint32           `json:"tapOwnerUid"`
	TapOwnerGID       *uint32           `json:"tapOwnerGid"`
	GuestAddress      string            `json:"guestAddress"`
	GuestMAC          string            `json:"guestMac"`
	IngressTCPPorts   []uint16          `json:"ingressTCPPorts"`
	ControlPlaneCIDRs []string          `json:"controlPlaneCIDRs"`
	RuntimeConfig     runtimeConfig     `json:"runtimeConfig"`
	RawPrevResult     map[string]any    `json:"prevResult"`
}

type runtimeConfig struct {
	DNS          runtimeDNS       `json:"dns"`
	PortMappings []json.RawMessage `json:"portMappings"`
}

type runtimeDNS struct {
	Servers []string `json:"servers"`
}

func Parse(data []byte) (*Config, error) {
	var raw rawConfig
	if err := json.Unmarshal(data, &raw); err != nil {
		return nil, fmt.Errorf("decode network configuration: %w", err)
	}

	if raw.CNIVersion != "1.0.0" {
		return nil, fmt.Errorf("unsupported CNI version %q", raw.CNIVersion)
	}

	if raw.Name == "" {
		return nil, fmt.Errorf("network name is required")
	}

	if raw.Type != "oas-vm-net" {
		return nil, fmt.Errorf("plugin type must be oas-vm-net, got %q", raw.Type)
	}

	if !interfaceNamePattern.MatchString(raw.TapName) {
		return nil, fmt.Errorf("invalid tap name %q", raw.TapName)
	}

	if raw.TapOwnerUID == nil || *raw.TapOwnerUID == 0 {
		return nil, fmt.Errorf("tapOwnerUid must be a non-zero UID")
	}

	if raw.TapOwnerGID == nil || *raw.TapOwnerGID == 0 {
		return nil, fmt.Errorf("tapOwnerGid must be a non-zero GID")
	}

	if len(raw.RuntimeConfig.PortMappings) != 0 {
		return nil, fmt.Errorf("hostPort/portMappings are unsupported")
	}

	tapAddress, err := parsePrivateIPv4Prefix("tapAddress", raw.TapAddress)
	if err != nil {
		return nil, err
	}

	guestAddress, err := parsePrivateIPv4Prefix("guestAddress", raw.GuestAddress)
	if err != nil {
		return nil, err
	}

	if tapAddress.Bits() != guestAddress.Bits() ||
		tapAddress.Masked() != guestAddress.Masked() {
		return nil, fmt.Errorf("tapAddress and guestAddress must be in the same subnet")
	}

	if tapAddress.Addr() == guestAddress.Addr() {
		return nil, fmt.Errorf("tapAddress and guestAddress must be different")
	}

	tapMAC, err := parseMAC("tapMac", raw.TapMAC)
	if err != nil {
		return nil, err
	}

	guestMAC, err := parseMAC("guestMac", raw.GuestMAC)
	if err != nil {
		return nil, err
	}

	if tapMAC.String() == guestMAC.String() {
		return nil, fmt.Errorf("tapMac and guestMac must be different")
	}

	ports, err := parsePorts(raw.IngressTCPPorts)
	if err != nil {
		return nil, err
	}

	controlCIDRs, err := parseControlCIDRs(raw.ControlPlaneCIDRs)
	if err != nil {
		return nil, err
	}

	dns, err := parseDNS(raw.RuntimeConfig.DNS.Servers)
	if err != nil {
		return nil, err
	}

	return &Config{
		CNIVersion:        raw.CNIVersion,
		Name:              raw.Name,
		Type:              raw.Type,
		TapName:           raw.TapName,
		TapAddress:        tapAddress,
		TapMAC:            tapMAC,
		TapOwnerUID:       *raw.TapOwnerUID,
		TapOwnerGID:       *raw.TapOwnerGID,
		GuestAddress:      guestAddress,
		GuestMAC:          guestMAC,
		IngressTCPPorts:   ports,
		ControlPlaneCIDRs: controlCIDRs,
		DNS:               dns,
		RawPrevResult:     raw.RawPrevResult,
	}, nil
}

func parsePrivateIPv4Prefix(field, value string) (netip.Prefix, error) {
	prefix, err := netip.ParsePrefix(value)
	if err != nil {
		return netip.Prefix{}, fmt.Errorf("%s: %w", field, err)
	}

	addr := prefix.Addr()
	if !addr.Is4() || !addr.IsPrivate() || addr.IsUnspecified() ||
		addr.IsMulticast() || addr.IsLoopback() {
		return netip.Prefix{}, fmt.Errorf("%s must be a private unicast IPv4 prefix", field)
	}

	if prefix.Bits() >= 31 {
		return netip.Prefix{}, fmt.Errorf("%s must leave at least two usable host addresses", field)
	}

	network := prefix.Masked().Addr()
	broadcast := broadcastAddress(prefix)
	if !network.IsPrivate() || !broadcast.IsPrivate() {
		return netip.Prefix{}, fmt.Errorf("%s subnet must be fully private", field)
	}

	if addr == network || addr == broadcast {
		return netip.Prefix{}, fmt.Errorf("%s must be a usable host address", field)
	}

	return prefix, nil
}

func broadcastAddress(prefix netip.Prefix) netip.Addr {
	bytes := prefix.Masked().Addr().As4()
	hostBits := uint(32 - prefix.Bits())
	value := uint32(bytes[0])<<24 |
		uint32(bytes[1])<<16 |
		uint32(bytes[2])<<8 |
		uint32(bytes[3])
	value |= uint32(1<<hostBits) - 1
	return netip.AddrFrom4([4]byte{
		byte(value >> 24),
		byte(value >> 16),
		byte(value >> 8),
		byte(value),
	})
}

func parseMAC(field, value string) (net.HardwareAddr, error) {
	mac, err := net.ParseMAC(value)
	if err != nil || len(mac) != 6 {
		return nil, fmt.Errorf("%s must be a 6-byte MAC address", field)
	}

	if mac[0]&1 != 0 {
		return nil, fmt.Errorf("%s must be a unicast MAC address", field)
	}

	if mac[0]&2 == 0 {
		return nil, fmt.Errorf("%s must be locally administered", field)
	}

	return mac, nil
}

func parsePorts(values []uint16) ([]uint16, error) {
	if len(values) == 0 {
		return nil, fmt.Errorf("ingressTCPPorts must include SSH port 22")
	}

	seen := make(map[uint16]struct{}, len(values))
	hasSSH := false
	for _, port := range values {
		if port == 0 {
			return nil, fmt.Errorf("ingressTCPPorts cannot contain port 0")
		}
		if _, ok := seen[port]; ok {
			return nil, fmt.Errorf("ingressTCPPorts contains duplicate port %d", port)
		}
		seen[port] = struct{}{}
		hasSSH = hasSSH || port == 22
	}

	if !hasSSH {
		return nil, fmt.Errorf("ingressTCPPorts must include SSH port 22")
	}

	out := slices.Clone(values)
	slices.Sort(out)
	return out, nil
}

func parseControlCIDRs(values []string) ([]netip.Prefix, error) {
	if len(values) == 0 {
		return nil, fmt.Errorf("controlPlaneCIDRs cannot be empty")
	}

	out := make([]netip.Prefix, 0, len(values))
	seen := make(map[netip.Prefix]struct{}, len(values))
	for _, value := range values {
		prefix, err := netip.ParsePrefix(value)
		if err != nil {
			return nil, fmt.Errorf("controlPlaneCIDRs: %w", err)
		}

		if !prefix.Addr().Is4() || prefix != prefix.Masked() {
			return nil, fmt.Errorf("controlPlaneCIDRs must contain canonical IPv4 prefixes")
		}

		if prefix.Bits() == 0 || !prefix.Addr().IsGlobalUnicast() {
			return nil, fmt.Errorf("controlPlaneCIDRs must contain bounded unicast IPv4 prefixes")
		}

		if _, ok := seen[prefix]; ok {
			return nil, fmt.Errorf("controlPlaneCIDRs contains duplicate %s", prefix)
		}

		seen[prefix] = struct{}{}
		out = append(out, prefix)
	}

	slices.SortFunc(out, func(a, b netip.Prefix) int {
		return a.Addr().Compare(b.Addr())
	})
	return out, nil
}

func parseDNS(values []string) (DNS, error) {
	var out DNS
	seen := make(map[netip.Addr]struct{}, len(values))
	for _, value := range values {
		addr, err := netip.ParseAddr(value)
		if err != nil || !addr.Is4() || !addr.IsGlobalUnicast() ||
			addr == netip.MustParseAddr("255.255.255.255") {
			return DNS{}, fmt.Errorf("runtime DNS servers must be unicast IPv4 addresses")
		}

		if _, ok := seen[addr]; ok {
			continue
		}

		seen[addr] = struct{}{}
		out.Servers = append(out.Servers, addr)
	}
	return out, nil
}