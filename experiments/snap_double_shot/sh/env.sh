export LOGFILE=./firecracker.log

# 这个要改为snap_double_shot所在的真实路径
export ROOTROOT=/home/yunfei/workspace/snap_double_shot

export FC=$ROOTROOT/bin/firecracker
export JAILER=$ROOTROOT/bin/jailer
export KERNEL=$ROOTROOT/vm_resourse/vmlinux
export ROOTFS=$ROOTROOT/vm_resourse/rootfs.ext4

export BASE=$ROOTROOT/work_cwd
export UID_FC=1234
export GID_FC=1234

export ID_A=vm-a
export ID_B=vm-b

export ROOT_A=$BASE/firecracker/$ID_A/root
export ROOT_B=$BASE/firecracker/$ID_B/root

export SOCK_A=$ROOT_A/run/firecracker.socket
export SOCK_B=$ROOT_B/run/firecracker.socket

export DATA_A=$ROOTROOT/vm_resourse/data-a.ext4
export DATA_B=$ROOTROOT/vm_resourse/data-b.ext4

export SNAP_VSTATE=$ROOTA/snapshot/vestate
export SNAP_BIOS=$ROOTA/snapshot/bsn