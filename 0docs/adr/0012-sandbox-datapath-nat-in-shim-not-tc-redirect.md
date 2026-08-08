---
status: accepted
---

# 沙箱网络数据面：oas-ctrd-shim 内做 L3 NAT，不移植 kata tc-redirect

对接 k8s+Calico 时，沙箱 netns 里只有 Calico 的 veth `eth0`(PodIP/32)，OAS 需要自己的 `tapH0` 喂给 firecracker virtio-net。kata 的 `tcfilter` 用 `mirred egress redirect` 在 `eth0`↔`tap` 间做 L2 桥接，初版设想直接移植。**决定：不做 tc-redirect，改在 `oas-ctrd-shim` 内做 L3 NAT**（DNAT PodIP→guestIP、SNAT guestIP→PodIP），tapH0 持网关 IP `172.16.0.1/30`、guest 保持固定 `172.16.0.2/30`。

根因是 **tc-redirect 与 NAT 在同一 tap 上互斥**：`mirred` 在 qdisc ingress 层 `TC_ACT_STOLEN` 窃包，早于 netfilter prerouting/forward/NAT 链，二者无法叠加。选谁由「guest 是否需要知晓 PodIP」决定——tc-redirect 是 L2 透明桥，guest 必须直接持有 PodIP 才能应答 ARP；NAT 则把 PodIP 翻译到 guest 固定 IP，guest 无需知晓 PodIP。

tc-redirect 路径要求 guest 在恢复后获知 PodIP，而 OAS 是 **firecracker snapshot restore（非 fresh boot）**：kernel 已在 vmstate 里活着，kernel cmdline `ip=`、init 脚本均不重跑；`GuestEndpoint::Vsock` 只是声明态 seam（`VmHandle.guest` 恒 `None`，bake 时未挂 vsock 设备），guest agent 是外部注入二进制、无配置通道。落地 tc-redirect 需新建 vsock 通道 + 扩展 guest agent + 一次性重 bake snapshot 加 vsock 设备——成本与本项目不匹配。NAT 路径零 guest 侧改动，控制面经 PodIP DNAT 即可达 guest，egress 经 SNAT 出网，满足「控制面主动连沙箱 + 沙箱可联网 + guest 不知 PodIP」的需求。

实现上 NAT 从 `oas-vm-net` Go CNI 插件迁入 `oas-ctrd-shim`（Rust，shell out `ip netns exec` + `nft`，复用 Path A `oas-net` 的 shell-out 风格）：`VmCore::create` 之前于 pod netns 内幂等创建 tapH0 + 装 nft 规则，`Stop` 时杀 firecracker 后幂等拆除。`oas-vm-net` 保留代码不删除但不再是活路径。共享 `vm_core` 不动，Path A（`oas-net`）不受影响。MVP 仅做全端口 DNAT/SNAT + 双向 forward，`oas_guard`（netdev MAC 过滤）与 `IngressTCPPorts`（端口 DNAT）延后。vsock CID 在 snapshot restore 下不可 per-pod 重配的问题随 tc-redirect 一并搁置。
