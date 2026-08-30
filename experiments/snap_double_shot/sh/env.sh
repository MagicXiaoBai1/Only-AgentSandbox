export LOGFILE=./firecracker.log

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

export SNAP_VMSTATE=$ROOTROOT/snapshot/vmstate
export SNAP_MEM=$ROOTROOT/snapshot/mem

export TAP_DEV="tapH1"
TAP_IP="172.16.0.1"
MASK_SHORT="/30"
export FC_MAC="06:00:AC:10:00:02"

export NETNS_A_NAME="vmnsH"
export NETNS_B_NAME="vmnsH2"
