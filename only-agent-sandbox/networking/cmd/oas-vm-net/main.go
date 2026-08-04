package main

import (
	"fmt"
	"os"

	"github.com/containernetworking/cni/pkg/skel"
	"github.com/containernetworking/cni/pkg/types"
	"github.com/containernetworking/cni/pkg/version"

	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/dataplane"
	"github.com/MagicXiaoBai1/Only-AgentSandbox/only-agent-sandbox/networking/internal/plugin"
)

var runner *plugin.Runner

func main() {
	adapter := dataplane.NewLinuxAdapter()
	runner = plugin.New(adapter)
	skel.PluginMain(
		cmdAdd,
		cmdCheck,
		cmdDel,
		version.PluginSupports("1.0.0"),
		"oas-vm-net: bridge fixed Firecracker TAP networking to a CNI-assigned IPv4",
	)
}

func cmdAdd(args *skel.CmdArgs) error {
	result, err := runner.Add(args)
	if err != nil {
		return err
	}
	return types.PrintResult(result, result.CNIVersion)
}

func cmdCheck(args *skel.CmdArgs) error {
	return runner.Check(args)
}

func cmdDel(args *skel.CmdArgs) error {
	if err := runner.Delete(args); err != nil {
		// Local TAP/nft resources disappear with the netns. Do not block the
		// following Calico DEL, which owns WEP and IPAM cleanup.
		_, _ = fmt.Fprintf(os.Stderr, "oas-vm-net: best-effort DEL: %v\n", err)
	}
	return nil
}