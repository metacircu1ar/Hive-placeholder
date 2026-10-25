//! Reject known path conflicts before provisioning changes any managed object.
use super::{Path, Plan, Result, files, invalid};
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn directory(path: &Path) -> Result<()> {
    if crate::disk::exists(path)? {
        files::owned(path, true)?;
    }
    Ok(())
}
pub(super) fn socket_parent(path: &Path) -> Result<()> {
    use crate::service::socket::directory::{Action, check};
    check(path, Action::AllowMissing)?;
    Ok(())
}
pub(super) fn paths(plan: &Plan) -> Result<()> {
    hive_setup_support::names::check_output_name(&plan.socket)?;
    if let Some(bridge) = &plan.bridge_file {
        hive_setup_support::names::check_output_name(bridge)?;
    }
    directory(&plan.root)?;
    let admin = plan.root.join(".comb");
    if plan.root.starts_with(&plan.socket)
        || (plan.socket.starts_with(&admin) && plan.socket != admin.join("service.sock"))
    {
        return Err(invalid(
            "Knowledge root and socket must have separate, nonconflicting paths.",
        ));
    }
    socket_parent(
        plan.socket
            .parent()
            .ok_or_else(|| invalid("Socket needs a parent."))?,
    )?;
    if crate::disk::exists(&plan.socket)? {
        comb_protocol::service::check_socket(&plan.socket)?;
        let fd = rustix::net::socket(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::STREAM,
            None,
        )
        .map_err(std::io::Error::from)?;
        rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC).map_err(std::io::Error::from)?;
        let stream = std::os::unix::net::UnixStream::from(fd);
        stream.set_nonblocking(true)?;
        let address =
            rustix::net::SocketAddrUnix::new(&plan.socket).map_err(std::io::Error::from)?;
        match rustix::net::connect(&stream, &address) {
            Err(e) if e == rustix::io::Errno::CONNREFUSED => (),
            Err(e) => return Err(std::io::Error::from(e).into()),
            Ok(()) => {
                if !crate::services::owned(&plan.root)? {
                    return Err(invalid(
                        "Socket is serving a live instance; stop it before setup.",
                    ));
                }
            }
        }
    }
    if let Some(bridge) = &plan.bridge_file {
        if overlaps(bridge, &plan.socket)
            || plan.root.starts_with(bridge)
            || bridge.starts_with(&admin)
        {
            return Err(invalid(
                "Bridge file conflicts with the socket or knowledge administration.",
            ));
        }
        directory(
            bridge
                .parent()
                .ok_or_else(|| invalid("Bridge needs a parent."))?,
        )?;
    }
    Ok(())
}
