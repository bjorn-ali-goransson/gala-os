//! The standard ws2_32 ordinals (the Winsock 1.1 numbering every ws2_32
//! keeps): programs import these functions by ordinal, so the exports
//! carry them.
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("accept", 1),
        ("bind", 2),
        ("closesocket", 3),
        ("connect", 4),
        ("getpeername", 5),
        ("getsockname", 6),
        ("getsockopt", 7),
        ("htonl", 8),
        ("htons", 9),
        ("ioctlsocket", 10),
        ("inet_addr", 11),
        ("inet_ntoa", 12),
        ("listen", 13),
        ("ntohl", 14),
        ("ntohs", 15),
        ("recv", 16),
        ("recvfrom", 17),
        ("select", 18),
        ("send", 19),
        ("sendto", 20),
        ("setsockopt", 21),
        ("shutdown", 22),
        ("socket", 23),
        ("gethostbyaddr", 51),
        ("gethostbyname", 52),
        ("getservbyport", 56),
        ("getservbyname", 55),
        ("gethostname", 57),
        ("WSAGetLastError", 111),
        ("WSASetLastError", 112),
        ("WSAStartup", 115),
        ("WSACleanup", 116),
        ("__WSAFDIsSet", 151),
    ];
    // (rustc's .def names them too: lld warns about a duplicate export and
    // keeps this one, with its ordinal.)
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
