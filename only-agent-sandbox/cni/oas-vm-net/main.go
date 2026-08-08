// oas-vm-net — CNI chained plugin: fixed tap + G↔P 1:1 NAT for Firecracker OAS.
//
// Expected position: last plugin in a conflist after Calico (or lab ptp/host-local).
// prevResult must already place Pod IP P on the sandbox netns interface (usually eth0).
//
// ADD:
//   1. Create tapH0 with T=172.16.0.1/30 (configurable)
//   2. Enable IPv4 forward in the netns
//   3. nft table ip oas_vm: SNAT G→P, DNAT P→G (all ports; guest_agent :10000 included)
// DEL: remove nft table + tap (netns deletion by runtime is also fine)
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"

	"github.com/containernetworking/cni/pkg/skel"
	"github.com/containernetworking/cni/pkg/types"
	current "github.com/containernetworking/cni/pkg/types/100"
	"github.com/containernetworking/cni/pkg/version"
	"github.com/containernetworking/plugins/pkg/ns"
	bv "github.com/containernetworking/plugins/pkg/utils/buildversion"
)

type NetConf struct {
	types.NetConf

	TapName    string `json:"tapName"`
	TapGateway string `json:"tapGateway"` // CIDR, e.g. 172.16.0.1/30
	GuestIP    string `json:"guestIP"`    // bare IP, e.g. 172.16.0.2
	GuestMAC   string `json:"guestMAC,omitempty"`
	// DNSServer optional: redirect UDP/TCP T:53 → DNSServer:53 (CRI DNS).
	DNSServer string `json:"dnsServer,omitempty"`
}

func loadConf(bytes []byte) (*NetConf, *current.Result, error) {
	conf := &NetConf{
		TapName:    "tapH0",
		TapGateway: "172.16.0.1/30",
		GuestIP:    "172.16.0.2",
	}
	if err := json.Unmarshal(bytes, conf); err != nil {
		return nil, nil, fmt.Errorf("parse conf: %w", err)
	}
	if conf.RawPrevResult == nil {
		return nil, nil, errors.New("oas-vm-net must be chained; missing prevResult")
	}
	if err := version.ParsePrevResult(&conf.NetConf); err != nil {
		return nil, nil, fmt.Errorf("parsePrevResult: %w", err)
	}
	prev, err := current.NewResultFromResult(conf.PrevResult)
	if err != nil {
		return nil, nil, fmt.Errorf("convert prevResult: %w", err)
	}
	return conf, prev, nil
}

func podIPFromResult(result *current.Result) (net.IP, error) {
	for _, ip := range result.IPs {
		if ip.Address.IP.To4() == nil {
			continue
		}
		return ip.Address.IP.To4(), nil
	}
	return nil, errors.New("no IPv4 in prevResult")
}

func cmdAdd(args *skel.CmdArgs) error {
	conf, prev, err := loadConf(args.StdinData)
	if err != nil {
		return err
	}
	podIP, err := podIPFromResult(prev)
	if err != nil {
		return err
	}
	guestIP := net.ParseIP(conf.GuestIP).To4()
	if guestIP == nil {
		return fmt.Errorf("invalid guestIP %q", conf.GuestIP)
	}
	tapGW, tapNet, err := net.ParseCIDR(conf.TapGateway)
	if err != nil {
		return fmt.Errorf("invalid tapGateway %q: %w", conf.TapGateway, err)
	}
	tapGW = tapGW.To4()
	if tapGW == nil {
		return fmt.Errorf("tapGateway must be IPv4 CIDR")
	}

	netns, err := ns.GetNS(args.Netns)
	if err != nil {
		return fmt.Errorf("get netns %s: %w", args.Netns, err)
	}
	defer netns.Close()

	err = netns.Do(func(_ ns.NetNS) error {
		if err := ensureTap(conf.TapName, tapGW, tapNet); err != nil {
			return err
		}
		if err := enableForward(); err != nil {
			return err
		}
		return setupNAT(podIP, guestIP, tapGW, conf.DNSServer)
	})
	if err != nil {
		return err
	}

	return types.PrintResult(prev, conf.CNIVersion)
}

func cmdDel(args *skel.CmdArgs) error {
	conf, _, err := loadConf(args.StdinData)
	// DEL should be idempotent even with broken stdin; best-effort.
	if err != nil {
		conf = &NetConf{TapName: "tapH0"}
		_ = json.Unmarshal(args.StdinData, conf)
		if conf.TapName == "" {
			conf.TapName = "tapH0"
		}
	}
	if args.Netns == "" {
		return nil
	}
	netns, err := ns.GetNS(args.Netns)
	if err != nil {
		// netns already gone
		return nil
	}
	defer netns.Close()
	_ = netns.Do(func(_ ns.NetNS) error {
		_ = teardownNAT()
		_ = deleteTap(conf.TapName)
		return nil
	})
	return nil
}

func cmdCheck(args *skel.CmdArgs) error {
	conf, prev, err := loadConf(args.StdinData)
	if err != nil {
		return err
	}
	podIP, err := podIPFromResult(prev)
	if err != nil {
		return err
	}
	netns, err := ns.GetNS(args.Netns)
	if err != nil {
		return err
	}
	defer netns.Close()
	return netns.Do(func(_ ns.NetNS) error {
		return checkDataPlane(conf.TapName, podIP, net.ParseIP(conf.GuestIP).To4())
	})
}

func main() {
	skel.PluginMainFuncs(skel.CNIFuncs{
		Add:   cmdAdd,
		Del:   cmdDel,
		Check: cmdCheck,
	}, version.All, bv.BuildString("oas-vm-net"))
}
