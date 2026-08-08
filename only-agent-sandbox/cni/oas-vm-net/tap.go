package main

import (
	"fmt"
	"net"

	"github.com/vishvananda/netlink"
)

func ensureTap(name string, gateway net.IP, network *net.IPNet) error {
	link, err := netlink.LinkByName(name)
	if err != nil {
		tap := &netlink.Tuntap{
			LinkAttrs: netlink.LinkAttrs{Name: name},
			Mode:      netlink.TUNTAP_MODE_TAP,
		}
		if err := netlink.LinkAdd(tap); err != nil {
			return fmt.Errorf("create tap %s: %w", name, err)
		}
		link, err = netlink.LinkByName(name)
		if err != nil {
			return fmt.Errorf("lookup tap %s after create: %w", name, err)
		}
	}

	addrs, err := netlink.AddrList(link, netlink.FAMILY_V4)
	if err != nil {
		return err
	}
	want := &netlink.Addr{IPNet: &net.IPNet{IP: gateway, Mask: network.Mask}}
	have := false
	for _, a := range addrs {
		if a.IP.Equal(gateway) {
			have = true
			break
		}
	}
	if !have {
		if err := netlink.AddrAdd(link, want); err != nil {
			return fmt.Errorf("addr add %s on %s: %w", want.IPNet, name, err)
		}
	}
	if err := netlink.LinkSetUp(link); err != nil {
		return fmt.Errorf("link up %s: %w", name, err)
	}
	return nil
}

func deleteTap(name string) error {
	link, err := netlink.LinkByName(name)
	if err != nil {
		return nil
	}
	return netlink.LinkDel(link)
}

func checkDataPlane(tapName string, podIP, guestIP net.IP) error {
	if _, err := netlink.LinkByName(tapName); err != nil {
		return fmt.Errorf("tap missing: %w", err)
	}
	if podIP == nil || guestIP == nil {
		return fmt.Errorf("missing pod/guest ip for check")
	}
	return nil
}

func enableForward() error {
	return osWriteFile("/proc/sys/net/ipv4/ip_forward", []byte("1"))
}
