# 基于vsock的带外 agent 重构
目前这块的协议设计是否模糊，也影响到了`oas-manager`。现在要细化这块的设计。

硬限制单个沙箱仅对应单个容器，不支持单沙箱多容器

vm内外的协议暂定如下，但是MVP阶段不用实现：
Guest -> Host: Hello { sandbox_id, agent_version, boot_id }
Host -> Guest: RandomSeed { seed }
Host -> Guest: Start { command, args, env, cwd }
Guest -> Host: Started { pid, started_at }
Host -> Guest: Stop { timeout_sec }
Guest -> Host: Exited { exit_code, finished_at, reason }
Guest -> Host: Heartbeat { workload_state, healthy }

# 沙箱内外交流节点：
only agent sandbox进程内，因该做到`oas-driver`而不是`oas-manager`中，因为未来不同的虚拟机管理（vmm）可能要使用不同的VM内外交流方式，所以`oas-manager`与VM内部交流一定要经过`oas-driver`，`oas-driver`一定要对外屏蔽这种复杂度
当kubelet调用如下接口时
- CreateContainer 
- StartContainer  
- StopContainer   
- RemoveContainer 
- ContainerStatus 
- ListContainers 
请求到`oas-manager`然后到`oas-driver`

`oas-driver`仅对上暴露:
- createContainer()
- startContainer()
- stopContainer()
- removeContainer()
- getContainerStatus()

在 MVP 阶段，container这一层可以不用做，VM的snapshot恢复后即刻启动服务，服务甚至不用跑在容器中，直接跑在VM中（因为现在的codex都在做cgroup等沙箱了，这种沙箱嵌套会出问题）

