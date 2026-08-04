package result

import (
	"encoding/json"
	"fmt"
	"net/netip"
	"slices"

	"github.com/containernetworking/cni/pkg/types"
	types100 "github.com/containernetworking/cni/pkg/types/100"
	"github.com/containernetworking/cni/pkg/version"
)

type Parsed struct {
	Result        *types100.Result
	PodIP         netip.Addr
	InterfaceIndex int
}

func Parse(raw map[string]any, ifName, netnsPath string) (*Parsed, error) {
	if raw == nil {
		return nil, fmt.Errorf("prevResult is required")
	}

	conf := &types.PluginConf{
		CNIVersion:   stringValue(raw, "cniVersion"),
		RawPrevResult: cloneMap(raw),
	}

	if conf.CNIVersion == "" {
		return nil, fmt.Errorf("prevResult.cniVersion is required")
	}

	if err := version.ParsePrevResult(conf); err != nil {
		return nil, err
	}

	current, err := types100.NewResultFromResult(conf.PrevResult)
	if err != nil {
		return nil, fmt.Errorf("convert prevResult: %w", err)
	}

	matching := make([]int, 0, 1)
	for index, iface := range current.Interfaces {
		if iface == nil || iface.Name != ifName {
			continue
		}

		if iface.Sandbox != netnsPath {
			return nil, fmt.Errorf(
				"prevResult interface %q belongs to sandbox %q, want %q",
				ifName, iface.Sandbox, netnsPath,
			)
		}

		matching = append(matching, index)
	}

	if len(matching) > 1 {
		return nil, fmt.Errorf(
			"prevResult contains multiple interfaces named %q in sandbox %q",
			ifName, netnsPath,
		)
	}

	interfaceIndex := -1
	if len(matching) == 1 {
		interfaceIndex = matching[0]
	}

	var ipv4 []netip.Addr
	var ipv6 []netip.Addr
	for _, ipConfig := range current.IPs {
		if ipConfig == nil {
			return nil, fmt.Errorf("prevResult contains a nil IP configuration")
		}

		if ipConfig.Interface != nil {
			index := *ipConfig.Interface
			if index < 0 || index >= len(current.Interfaces) {
				return nil, fmt.Errorf("prevResult IP references invalid interface index %d", index)
			}
			if interfaceIndex == -1 || index != interfaceIndex {
				continue
			}
		} else if interfaceIndex != -1 {
			return nil, fmt.Errorf("prevResult IP for %q must reference its interface", ifName)
		}

		if ipv4Bytes := ipConfig.Address.IP.To4(); ipv4Bytes != nil {
			addr, ok := netip.AddrFromSlice(ipv4Bytes)
			if !ok {
				return nil, fmt.Errorf("invalid IPv4 address in prevResult")
			}
			ones, bits := ipConfig.Address.Mask.Size()
			if bits != 32 || ones != 32 {
				return nil, fmt.Errorf("Calico IPv4 address must use /32, got %s", ipConfig.Address.String())
			}
			ipv4 = append(ipv4, addr)
		} else {
			addr, ok := netip.AddrFromSlice(ipConfig.Address.IP)
			if !ok {
				return nil, fmt.Errorf("invalid IP address in prevResult")
			}
			ipv6 = append(ipv6, addr)
		}
	}

	if len(ipv6) != 0 {
		return nil, fmt.Errorf("IPv6 and dual-stack results are unsupported")
	}
	if len(ipv4) != 1 {
		return nil, fmt.Errorf(
			"interface %q must have exactly one IPv4 address, got %d",
			ifName, len(ipv4),
		)
	}

	if interfaceIndex == -1 {
		for _, ipConfig := range current.IPs {
			if ipConfig != nil && ipConfig.Interface != nil {
				return nil, fmt.Errorf(
					"legacy prevResult without %q cannot contain indexed IPs",
					ifName,
				)
			}
		}
	}

	return &Parsed{
		Result:         current,
		PodIP:          ipv4[0],
		InterfaceIndex: interfaceIndex,
	}, nil
}

func AppendTap(parsed *Parsed, name, mac, sandbox string, mtu int) (*types100.Result, error) {
	if parsed == nil || parsed.Result == nil {
		return nil, fmt.Errorf("parsed result is required")
	}

	result := &types100.Result{
		CNIVersion: parsed.Result.CNIVersion,
		Interfaces: slices.Clone(parsed.Result.Interfaces),
		IPs:        slices.Clone(parsed.Result.IPs),
		Routes:     slices.Clone(parsed.Result.Routes),
		DNS:        parsed.Result.DNS,
	}

	result.Interfaces = append(result.Interfaces, &types100.Interface{
		Name:    name,
		Mac:     mac,
		Sandbox: sandbox,
		Mtu:     mtu,
	})

	return result, nil
}

func stringValue(values map[string]any, key string) string {
	value, _ := values[key].(string)
	return value
}

func cloneMap(in map[string]any) map[string]any {
	data, _ := json.Marshal(in)
	var out map[string]any
	_ = json.Unmarshal(data, &out)
	return out
}