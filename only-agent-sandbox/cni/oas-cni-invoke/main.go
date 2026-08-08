// oas-cni-invoke — thin libcni wrapper for OAS Rust NetManager.
//
// Usage:
//
//	oas-cni-invoke add  --config /path/to/net.conflist --netns /var/run/netns/oas-SID --id SID [--ifname eth0]
//	oas-cni-invoke del  --config ... --netns ... --id ... [--ifname eth0]
//	oas-cni-invoke check --config ... --netns ... --id ...
//
// Prints CNI result JSON on stdout for add/check.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/containernetworking/cni/libcni"
)

func main() {
	if len(os.Args) < 2 {
		fatal("usage: oas-cni-invoke <add|del|check> [flags]")
	}
	cmd := os.Args[1]
	fs := flag.NewFlagSet(cmd, flag.ExitOnError)
	config := fs.String("config", "", "path to .conflist")
	netns := fs.String("netns", "", "path to netns (/var/run/netns/...)")
	id := fs.String("id", "", "container/sandbox id")
	ifname := fs.String("ifname", "eth0", "interface name inside netns")
	cniPath := fs.String("cni-path", "/opt/cni/bin", "CNI plugin directory (colon-separated ok)")
	_ = fs.Parse(os.Args[2:])

	if *config == "" || *netns == "" || *id == "" {
		fatal("--config, --netns, --id are required")
	}

	netConf, err := libcni.ConfListFromFile(*config)
	if err != nil {
		fatal("load conflist: %v", err)
	}
	cni := libcni.NewCNIConfig(filepath.SplitList(*cniPath), nil)
	rt := &libcni.RuntimeConf{
		ContainerID: *id,
		NetNS:       *netns,
		IfName:      *ifname,
	}

	switch cmd {
	case "add":
		res, err := cni.AddNetworkList(netConf, rt)
		if err != nil {
			fatal("ADD: %v", err)
		}
		b, err := json.Marshal(res)
		if err != nil {
			fatal("marshal: %v", err)
		}
		os.Stdout.Write(b)
		os.Stdout.Write([]byte("\n"))
	case "del":
		if err := cni.DelNetworkList(netConf, rt); err != nil {
			fatal("DEL: %v", err)
		}
	case "check":
		if err := cni.CheckNetworkList(netConf, rt); err != nil {
			fatal("CHECK: %v", err)
		}
		fmt.Println(`{"ok":true}`)
	default:
		fatal("unknown command %q", cmd)
	}
}

func fatal(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "oas-cni-invoke: "+format+"\n", args...)
	os.Exit(1)
}
