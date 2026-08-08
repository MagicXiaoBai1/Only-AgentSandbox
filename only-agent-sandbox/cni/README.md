# OAS CNI (方案 A1)

| 文件 | 说明 |
|------|------|
| `oas-vm-net/oas-vm-net` | 级联插件：固定 `tapH0` + nft `oas_vm` G↔P NAT |
| `oas-cni-invoke/oas-cni-invoke` | 最小 conflist 执行器（供 OAS Rust 调用） |
| `conflist/10-oas-lab.conflist` | 离线 lab：ptp + host-local + oas-vm-net |
| `conflist/10-oas-calico.conflist.example` | Calico 生产模板 |
| `smoke_lab.sh` | 无需切 k3s / 无需 FC 的 ADD+DNAT 冒烟 |
| `install.sh` | 安装到 `/opt/cni/bin` |

```bash
sudo ./install.sh
sudo ./smoke_lab.sh          # 无 FC：ptp + DNAT 冒烟
sudo CARGO_TARGET_DIR=$PWD/../target-calico ./smoke_fc_cni.sh   # FC+guest+PodIP
```

`smoke_fc_cni.sh`：**不经过 kubelet**（联调 §9 回滚后 k3s=containerd、通常也无 standalone kubelet）。  
只用 `/run/oas-calico.sock`，不动 `/run/oas.sock`。

Go 源码草稿（`oas-vm-net/*.go`）未接入构建；当前以 bash 插件为准。
