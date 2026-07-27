//! Shim 烟雾测试客户端：连一个**已运行**的 shim，调 Create/State/Stop，验证 snapshot 恢复。
//!
//! 这不是单元测试——它驱动真实 shim 二进制 + 真实 firecracker。用法见 `tools/test_shim.sh`。
//!
//! 用法：
//!   shim_smoke create <socket> <config> <sid> <bundle_dir> [rw_layer_path]
//!   shim_smoke state  <socket> <sid>
//!   shim_smoke stop   <socket> <sid>
//!
//! `create` 从 `config` 读 uid/gid/路径等，netns/tap 由 `Config::netns_path`/`net.tap_name` 派生
//! （与 runtime 侧一致），故调用方只需保证 netns + tap0 已建好（test_shim.sh 负责）。

use std::path::Path;
use std::time::Instant;

use oas_config::Config;
use oas_driver::generated::shim::{CreateRequest, StateRequest, StopRequest};
use oas_driver::generated::shim_ttrpc::ShimClient;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage:\n  {0} create <socket> <config> <sid> <bundle_dir> [rw_layer]\n  {0} state <socket> <sid>\n  {0} stop <socket> <sid>",
            args[0]
        );
        std::process::exit(2);
    }
    let cmd = &args[1];
    let socket = &args[2];
    let client = ttrpc::Client::connect(&format!("unix://{socket}"))?;
    let sc = ShimClient::new(client);

    match cmd.as_str() {
        "create" => {
            let cfg_path = args.get(3).expect("config path");
            let sid = args.get(4).expect("sandbox_id");
            let bundle_dir = args.get(5).expect("bundle_dir");
            let rw = args.get(6).cloned().unwrap_or_default();
            let cfg = Config::load(Path::new(cfg_path));
            let req = CreateRequest {
                sandbox_id: sid.clone(),
                bundle_dir: bundle_dir.clone(),
                netns_path: cfg.netns_path(sid).to_string_lossy().into_owned(),
                tap_name: cfg.net.tap_name.clone(),
                rw_layer_path: rw,
                cloud_disk_dev: String::new(),
                jailer_uid: cfg.jailer_uid,
                jailer_gid: cfg.jailer_gid,
                chroot_base_dir: cfg.chroot_base_dir.to_string_lossy().into_owned(),
                firecracker_bin: cfg.firecracker_bin.to_string_lossy().into_owned(),
                jailer_bin: cfg.jailer_bin.to_string_lossy().into_owned(),
                ..Default::default()
            };
            let t = Instant::now();
            let resp = sc.create(ttrpc::context::Context::default(), &req)?;
            let elapsed_ms = t.elapsed().as_millis();
            println!("create -> state={} error={} (rtt={}ms)", resp.state, resp.error, elapsed_ms);
            if resp.state != "Running" {
                std::process::exit(1);
            }
        }
        "state" => {
            let sid = args.get(3).expect("sid").clone();
            let t = Instant::now();
            let resp = sc.state(
                ttrpc::context::Context::default(),
                &StateRequest {
                    sandbox_id: sid,
                    ..Default::default()
                },
            )?;
            println!("state -> {} (rtt={}ms)", resp.state, t.elapsed().as_millis());
        }
        "stop" => {
            let sid = args.get(3).expect("sid").clone();
            sc.stop(
                ttrpc::context::Context::default(),
                &StopRequest {
                    sandbox_id: sid,
                    ..Default::default()
                },
            )?;
            println!("stop -> ok");
        }
        other => {
            eprintln!("unknown cmd: {other}");
            std::process::exit(2);
        }
    }
    Ok(())
}
