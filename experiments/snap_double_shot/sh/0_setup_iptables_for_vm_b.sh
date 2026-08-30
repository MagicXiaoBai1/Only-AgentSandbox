

ip netns add "$NETNS_B_NAME" 2>/dev/null || true

# Setup network interface
ip link del "$TAP_DEV" 2> /dev/null || true
ip tuntap add dev "$TAP_DEV" mode tap
ip link set "$TAP_DEV" netns "$NETNS_B_NAME"
ip netns exec "$NETNS_B_NAME" bash -c "
    # 给tap分配IP
    ip addr add ${TAP_IP}${MASK_SHORT} dev $TAP_DEV
    # 启用网卡
    ip link set dev $TAP_DEV up
"

# Enable ip forwarding
sh -c "echo 1 > /proc/sys/net/ipv4/ip_forward"

# Make sure the nat/MASQUERADE modules are loaded, otherwise the first
# iptables -t nat ... -j MASQUERADE call fails with
# "No chain/target/match by that name" on a fresh boot.
modprobe iptable_nat 2>/dev/null || true
modprobe xt_MASQUERADE 2>/dev/null || true

iptables -P FORWARD ACCEPT

# This tries to determine the name of the host network interface to forward
# VM's outbound network traffic through. If outbound traffic doesn't work,
# double check this returns the correct interface!
HOST_IFACE=$(ip -j route list default |jq -r '.[0].dev')

# Set up microVM internet access
iptables -t nat -D POSTROUTING -o "$HOST_IFACE" -j MASQUERADE || true
iptables -t nat -A POSTROUTING -o "$HOST_IFACE" -j MASQUERADE