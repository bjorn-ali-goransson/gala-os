//! The LDAP client functions PostgreSQL imports (its LDAP authentication
//! and libpq's ldap:// service lookups), none implemented yet
//! (`gala_stubs`): calling one reports `dll!function` to the ledger and
//! ends the process.

gala_stubs::unimplemented_exports!("wldap32.dll":
    ldap_unbind
    LdapGetLastError
    ldap_err2string
    ldap_first_entry
    ldap_get_dn
    ldap_get_values_len
    ldap_count_entries
    ldap_msgfree
    ldap_search_s
    ldap_search_st
    ldap_simple_bind_s
    ldap_value_free_len
    ldap_connect
    ldap_get_option
    ldap_init
    ldap_memfree
    ldap_set_option
    ldap_sslinit
    ldap_start_tls_sA
);
