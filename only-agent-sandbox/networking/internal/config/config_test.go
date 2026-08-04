package config

import (
	"strings"
	"testing"
)

// validConfigJSON is a minimal, fully valid OAS plugin configuration. Tests
// clone it and mutate single fields to exercise validation paths.
const validConfigJSON = `{
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
  "ingressTCPPorts": [22],
  "controlPlaneCIDRs": ["10.20.0.0/16"],
  "runtimeConfig": {"dns": {"servers": ["8.8.8.8", "1.1.1.1"]}}
}`

func mustParse(tb testing.TB, json string) *Config {
	tb.Helper()
	conf, err := Parse([]byte(json))
	if err != nil {
		tb.Fatalf("unexpected parse error: %v", err)
	}
	return conf
}

func TestParse_OK(t *testing.T) {
	conf := mustParse(t, validConfigJSON)

	if conf.CNIVersion != "1.0.0" {
		t.Fatalf("cniVersion = %q", conf.CNIVersion)
	}
	if conf.TapName != "tapH0" {
		t.Fatalf("tapName = %q", conf.TapName)
	}
	if conf.TapAddress.Addr().String() != "172.16.0.1" || conf.TapAddress.Bits() != 30 {
		t.Fatalf("tapAddress = %s", conf.TapAddress)
	}
	if conf.TapOwnerUID != 1234 || conf.TapOwnerGID != 1234 {
		t.Fatalf("owner uid/gid = %d/%d", conf.TapOwnerUID, conf.TapOwnerGID)
	}
	if len(conf.IngressTCPPorts) != 1 || conf.IngressTCPPorts[0] != 22 {
		t.Fatalf("ingress ports = %v", conf.IngressTCPPorts)
	}
	if len(conf.DNS.Servers) != 2 {
		t.Fatalf("dns servers = %v", conf.DNS.Servers)
	}
	if conf.DNS.Servers[0].String() != "8.8.8.8" {
		t.Fatalf("first dns server = %s", conf.DNS.Servers[0])
	}
}

func TestParse_Rejects(t *testing.T) {
	cases := []struct {
		name    string
		mutate  func(string) string
		wantSub string
	}{
		{"bad cni version", func(s string) string { return strings.Replace(s, "1.0.0", "0.4.0", 1) }, "unsupported CNI version"},
		{"missing name", func(s string) string { return strings.Replace(s, `"name": "k8s-pod-network"`, `"name": ""`, 1) }, "network name is required"},
		{"wrong type", func(s string) string { return strings.Replace(s, `"oas-vm-net"`, `"bridge"`, 1) }, "plugin type must be oas-vm-net"},
		{"bad tap name", func(s string) string { return strings.Replace(s, `"tapH0"`, `"bad tap!"`, 1) }, "invalid tap name"},
		{"zero owner uid", func(s string) string { return strings.Replace(s, `"tapOwnerUid": 1234`, `"tapOwnerUid": 0`, 1) }, "tapOwnerUid must be a non-zero UID"},
		{"port mappings", func(s string) string {
			return strings.Replace(s, `"runtimeConfig": {"dns": {"servers": ["8.8.8.8", "1.1.1.1"]}}`,
				`"runtimeConfig": {"dns": {"servers": ["8.8.8.8"]}, "portMappings": [{}]}`, 1)
		}, "portMappings are unsupported"},
		{"tap/guest different subnet", func(s string) string {
			return strings.Replace(s, `"guestAddress": "172.16.0.2/30"`, `"guestAddress": "172.16.1.2/30"`, 1)
		}, "same subnet"},
		{"tap equals guest addr", func(s string) string {
			return strings.Replace(s, `"guestAddress": "172.16.0.2/30"`, `"guestAddress": "172.16.0.1/30"`, 1)
		}, "must be different"},
		{"duplicate ports", func(s string) string {
			return strings.Replace(s, `"ingressTCPPorts": [22]`, `"ingressTCPPorts": [22, 22]`, 1)
		}, "duplicate port"},
		{"missing ssh port", func(s string) string {
			return strings.Replace(s, `"ingressTCPPorts": [22]`, `"ingressTCPPorts": [80]`, 1)
		}, "SSH port 22"},
		{"empty control cidrs", func(s string) string {
			return strings.Replace(s, `"controlPlaneCIDRs": ["10.20.0.0/16"]`, `"controlPlaneCIDRs": []`, 1)
		}, "controlPlaneCIDRs cannot be empty"},
		{"non-canonical control cidr", func(s string) string {
			return strings.Replace(s, `"10.20.0.0/16"`, `"10.20.0.5/16"`, 1)
		}, "canonical"},
		{"globally non-unicast control cidr", func(s string) string {
			return strings.Replace(s, `"10.20.0.0/16"`, `"224.0.0.0/4"`, 1)
		}, "bounded unicast"},
		{"tap address not private", func(s string) string {
			return strings.Replace(s, `"tapAddress": "172.16.0.1/30"`, `"tapAddress": "8.8.8.1/30"`, 1)
		}, "private unicast"},
		{"/31 tap prefix", func(s string) string {
			return strings.Replace(s, `"tapAddress": "172.16.0.1/30"`, `"tapAddress": "172.16.0.1/31"`, 1)
		}, "two usable host addresses"},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, err := Parse([]byte(tc.mutate(validConfigJSON)))
			if err == nil {
				t.Fatalf("expected error containing %q, got nil", tc.wantSub)
			}
			if !strings.Contains(err.Error(), tc.wantSub) {
				t.Fatalf("error %q does not contain %q", err.Error(), tc.wantSub)
			}
		})
	}
}

func TestParseMAC_LocallyAdministered(t *testing.T) {
	// 00:.. is globally unique (not locally administered) -> rejected.
	if _, err := parseMAC("tapMac", "00:11:22:33:44:55"); err == nil {
		t.Fatal("expected error for globally-unique MAC")
	}
	// 03:.. has multicast bit set -> rejected as not unicast.
	if _, err := parseMAC("tapMac", "03:00:00:00:00:01"); err == nil {
		t.Fatal("expected error for multicast MAC")
	}
	// 06:.. is locally administered unicast -> accepted.
	if _, err := parseMAC("tapMac", "06:00:ac:10:00:01"); err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestParsePrivateIPv4Prefix_Broadcast(t *testing.T) {
	// Network and broadcast addresses of the /30 are rejected.
	if _, err := parsePrivateIPv4Prefix("tapAddress", "172.16.0.0/30"); err == nil {
		t.Fatal("expected error for network address")
	}
	if _, err := parsePrivateIPv4Prefix("tapAddress", "172.16.0.3/30"); err == nil {
		t.Fatal("expected error for broadcast address")
	}
}
