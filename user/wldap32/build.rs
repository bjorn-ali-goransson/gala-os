//! Programs import wldap32's functions by ordinal only (PostgreSQL's
//! postgres.exe and libpq.dll). The numbers are those of Windows'
//! wldap32.dll, taken from Wine's dlls/wldap32/wldap32.spec (LGPL; the
//! numbers only, recorded in THIRD_PARTY.md).
fn main() {
    let ordinals: &[(&str, u32)] = &[
        ("ldap_unbind", 13),
        ("LdapGetLastError", 16),
        ("ldap_err2string", 22),
        ("ldap_first_entry", 26),
        ("ldap_get_dn", 30),
        ("ldap_get_values_len", 35),
        ("ldap_count_entries", 36),
        ("ldap_msgfree", 41),
        ("ldap_search_s", 50),
        ("ldap_search_st", 51),
        ("ldap_simple_bind_s", 60),
        ("ldap_value_free_len", 79),
        ("ldap_connect", 88),
        ("ldap_get_option", 136),
        ("ldap_init", 143),
        ("ldap_memfree", 200),
        ("ldap_set_option", 211),
        ("ldap_sslinit", 217),
        ("ldap_start_tls_sA", 330),
    ];
    for (name, ordinal) in ordinals {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:{name},@{ordinal}");
    }
}
