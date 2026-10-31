//! Export forwarders: mswsock's Microsoft extensions live in gala's ws2_32
//! (which also hands them out through WSAIoctl); these exports name them
//! there, and the loader follows them.
fn main() {
    for name in ["AcceptEx", "GetAcceptExSockaddrs"] {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name}=ws2_32.{name}");
    }
}
