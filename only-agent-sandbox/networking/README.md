# oas-vm-net

`oas-vm-net` is the final plugin in the OAS-specific Calico CNI chain. It
keeps the Firecracker guest network fixed while exposing the Calico-assigned
Pod IPv4 as the sandbox identity.

```text
guest 172.16.0.2/30
  <-> fixed tapH0 172.16.0.1/30
  <-> one-to-one SNAT/DNAT
  <-> Calico eth0 /32
```

The plugin:

- consumes Calico's `prevResult`;
- validates the assigned IPv4 against the real `CNI_IFNAME` in `CNI_NETNS`;
- creates a persistent TAP owned by the configured jailer UID/GID;
- enables IPv4 forwarding inside the sandbox netns;
- installs attachment-owned nftables NAT and filtering tables;
- preserves Calico's result and appends the TAP interface;
- checks for network drift with `CNI_COMMAND=CHECK`;
- deletes only resources carrying the attachment ownership marker.

## Limits

- Linux and IPv4 only;
- CNI specification `1.0.0`;
- one Calico IPv4 per sandbox;
- no hostPort/`portMappings`;
- no Calico eBPF dataplane validation yet;
- DNS uses the first IPv4 server in `runtimeConfig.dns.servers`.

The plugin must only appear in an OAS-specific CNI config directory. Do not add
it to the default Calico conflist used by ordinary Pods.

Calico must enable forwarding in the workload netns:

```json
"container_settings": {
  "allow_ip_forwarding": true
}
```

## Build And Test

```bash
make test
make check
make build
```

These commands do not start Firecracker. Unit tests cover config parsing,
prevResult handling and nftables-script rendering without privileges.

The end-to-end tests (`TestE2E_*` in `internal/plugin`) exercise the full
ADD/CHECK/DEL lifecycle against a real, isolated network namespace: they create
a named netns, install a veth as the Calico `eth0` with a /32 pod IP and a
default route, then run the plugin to create a persistent TAP, enable IPv4
forwarding and install the nftables NAT/filter tables. They require root
(`CAP_NET_ADMIN`) and the `nft` binary, and skip otherwise:

```bash
sudo -E go test -run TestE2E ./internal/plugin
```

## Installation

1. Build the binary and install it as `/opt/cni/bin/oas-vm-net`.
2. Copy `examples/10-calico-oas.conflist` to a dedicated directory such as
   `/etc/cni/net.d/oas/`.
3. Replace every placeholder in the example.
4. Point only the OAS runtime handler at that `cni_conf_dir`.

The Calico configuration and IPPool still own WorkloadEndpoint, policy, routing,
IPAM, and optional Internet `natOutgoing`.

As required by CNI, the runtime must set `CNI_PATH` for every command,
including `DEL`.