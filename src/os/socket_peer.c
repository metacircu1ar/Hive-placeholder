/* Keep Apple's opaque ABI structures in the system SDK, not mirrored in Rust. */
#include <libproc.h>
#include <sys/proc_info.h>
#include <sys/socket.h>
#include <unistd.h>

int hive_socket_peer(pid_t peer, int remote_fd, int local_fd) {
    struct socket_fdinfo local = {0}, remote = {0};
    if (proc_pidfdinfo(getpid(), local_fd, PROC_PIDFDSOCKETINFO,
                      &local, sizeof(local)) != sizeof(local) ||
        proc_pidfdinfo(peer, remote_fd, PROC_PIDFDSOCKETINFO,
                      &remote, sizeof(remote)) != sizeof(remote)) return 0;
    if (local.psi.soi_kind != SOCKINFO_UN || remote.psi.soi_kind != SOCKINFO_UN ||
        local.psi.soi_family != AF_UNIX || remote.psi.soi_family != AF_UNIX ||
        local.psi.soi_type != SOCK_STREAM || remote.psi.soi_type != SOCK_STREAM)
        return 0;
    return local.psi.soi_so != 0 && remote.psi.soi_so != 0 &&
        local.psi.soi_so == remote.psi.soi_proto.pri_un.unsi_conn_so &&
        remote.psi.soi_so == local.psi.soi_proto.pri_un.unsi_conn_so;
}
