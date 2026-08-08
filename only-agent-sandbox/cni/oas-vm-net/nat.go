package main

import (
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
)

const nftTable = "oas_vm"

// setupNAT installs 1:1 G↔P NAT inside the sandbox netns.
//
//	prerouting:  daddr P  → dnat to G
//	postrouting: saddr G  → snat to P
//
// Optional DNS: daddr T udp/tcp dport 53 → dnat to dnsServer:53
func setupNAT(podIP, guestIP, tapGW net.IP, dnsServer string) error {
	_ = teardownNAT()

	script := fmt.Sprintf(`
add table ip %s
add chain ip %s prerouting { type nat hook prerouting priority dstnat; policy accept; }
add chain ip %s postrouting { type nat hook postrouting priority srcnat; policy accept; }
add chain ip %s forward { type filter hook forward priority filter; policy accept; }
add rule ip %s prerouting ip daddr %s counter dnat to %s
add rule ip %s postrouting ip saddr %s counter snat to %s
add rule ip %s forward ct state established,related accept
add rule ip %s forward ip saddr %s accept
add rule ip %s forward ip daddr %s accept
`, nftTable, nftTable, nftTable, nftTable,
		nftTable, podIP.String(), guestIP.String(),
		nftTable, guestIP.String(), podIP.String(),
		nftTable,
		nftTable, guestIP.String(),
		nftTable, guestIP.String(),
	)

	if dnsServer != "" && tapGW != nil {
		script += fmt.Sprintf(`
add rule ip %s prerouting ip daddr %s udp dport 53 counter dnat to %s:53
add rule ip %s prerouting ip daddr %s tcp dport 53 counter dnat to %s:53
`, nftTable, tapGW.String(), dnsServer,
			nftTable, tapGW.String(), dnsServer)
	}

	return nftApply(script)
}

func teardownNAT() error {
	// idempotent
	_ = exec.Command("nft", "delete", "table", "ip", nftTable).Run()
	return nil
}

func nftApply(script string) error {
	cmd := exec.Command("nft", "-f", "-")
	cmd.Stdin = strings.NewReader(script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("nft apply: %w (%s)", err, strings.TrimSpace(string(out)))
	}
	return nil
}

func osWriteFile(path string, data []byte) error {
	return os.WriteFile(path, data, 0o644)
}
