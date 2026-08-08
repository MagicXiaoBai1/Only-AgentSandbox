---
status: accepted
---

# oas-ctrd-shim 配置引导：env + 固定路径 + Default

`oas-ctrd-shim` 由 containerd 经 Legacy start 协议拉起，`shim::parse` 只给出 `-namespace`/`-id`/`-address`/`-socket`/`-publish-binary`，**无法像 Path A 那样透传 `--config <path>`**（Path A 的 `RealDriver` 自行 spawn shim，可任意传参）。因此 shim 在 `run_server` 启动时按 `OAS_CONFIG` 环境变量 → 固定路径 `/etc/oas/config.toml` → `Config::default()` 的顺序解析配置，复用既有 `Config::load` 的「文件缺失回落 Default」容错。

`sandbox_id` 仍来自 containerd 的 `-id` flag。`netns_path` **优先用 containerd 在 bundle `config.json` 里提供的 sandbox netns**（CRI 路径：`linux.namespaces[type=network].path`）；仅当 bundle 未给（裸 `ctr run` / e2e）时才回落 `cfg.netns_path(sid)`。即对接 containerd 时以 containerd 的 netns 为准，shim 不自建 netns。无论 netns 是谁建的，进 shim 前里面须有 `cfg.net.tap_name`（tapH0）的 tap —— 这属于 deferred 的 tap 归属问题，不在本 ADR。拒绝从 OCI bundle 注解读配置路径：bundle 是 per-container 的，而 shim 是 per-sandbox 的，一个 shim 可能服务多个 bundle。
