/* libcurl's configuration on Rustux (by hand: curl's configure cannot run
 * its checks for another system). HTTP and HTTPS (mbedTLS), file: and
 * data: URLs, cookies, gzip; DNS on a thread of its own. */
#ifndef CURL_CONFIG_RUSTUX_H
#define CURL_CONFIG_RUSTUX_H

#define CURL_OS "x86_64-pc-rustux"
#define OS CURL_OS

/* What is left out */
#define CURL_DISABLE_DICT 1
#define CURL_DISABLE_FTP 1
#define CURL_DISABLE_GOPHER 1
#define CURL_DISABLE_IMAP 1
#define CURL_DISABLE_LDAP 1
#define CURL_DISABLE_LDAPS 1
#define CURL_DISABLE_MQTT 1
#define CURL_DISABLE_POP3 1
#define CURL_DISABLE_RTSP 1
#define CURL_DISABLE_SMB 1
#define CURL_DISABLE_SMTP 1
#define CURL_DISABLE_TELNET 1
#define CURL_DISABLE_TFTP 1
#define CURL_DISABLE_NTLM 1
#define CURL_DISABLE_KERBEROS_AUTH 1
#define CURL_DISABLE_NEGOTIATE_AUTH 1
#define CURL_DISABLE_AWS 1
#define CURL_DISABLE_NETRC 1
#define CURL_DISABLE_SOCKETPAIR 1
#define CURL_DISABLE_IPFS 1
#define CURL_DISABLE_WEBSOCKETS 1
#define CURL_DISABLE_ALTSVC 1
#define CURL_DISABLE_HSTS 1

/* TLS, compression, IPv6, the resolver thread */
#define USE_MBEDTLS 1
#define HAVE_MBEDTLS_DES_CRYPT_ECB 0
#define HAVE_LIBZ 1
#define HAVE_ZLIB_H 1
#define USE_IPV6 1
#define USE_THREADS_POSIX 1
#define HAVE_PTHREAD_H 1
#define CURL_CA_BUNDLE "/etc/ssl/certs/ca-certificates.crt"
#define CURL_CA_PATH "/etc/ssl/certs"

/* Headers */
#define HAVE_ARPA_INET_H 1
#define HAVE_FCNTL_H 1
#define HAVE_LOCALE_H 1
#define HAVE_NETDB_H 1
#define HAVE_NETINET_IN_H 1
#define HAVE_NETINET_TCP_H 1
#define HAVE_POLL_H 1
#define HAVE_PWD_H 1
#define HAVE_SIGNAL_H 1
#define HAVE_STDATOMIC_H 1
#define HAVE_STDBOOL_H 1
#define HAVE_STDINT_H 1
#define HAVE_STRINGS_H 1
#define HAVE_SYS_IOCTL_H 1
#define HAVE_SYS_SELECT_H 1
#define HAVE_SYS_SOCKET_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TIME_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_UIO_H 1
#define HAVE_SYS_UTSNAME_H 1
#define HAVE_UNISTD_H 1
#define HAVE_UTIME_H 1

/* Functions and types */
#define HAVE_ALARM 1
#define HAVE_ATOMIC 1
#define HAVE_BASENAME 1
#define HAVE_BOOL_T 1
#define HAVE_CLOCK_GETTIME_MONOTONIC 1
#define HAVE_DECL_FSEEKO 1
#define HAVE_FCNTL 1
#define HAVE_FCNTL_O_NONBLOCK 1
#define HAVE_FREEADDRINFO 1
#define HAVE_FSEEKO 1
#define HAVE_FTRUNCATE 1
#define HAVE_GETADDRINFO 1
#define HAVE_GETADDRINFO_THREADSAFE 1
#define HAVE_GETEUID 1
#define HAVE_GETHOSTNAME 1
#define HAVE_GETPEERNAME 1
#define HAVE_GETPPID 1
#define HAVE_GETPWUID 1
#define HAVE_GETPWUID_R 1
#define HAVE_GETSOCKNAME 1
#define HAVE_GETTIMEOFDAY 1
#define HAVE_GMTIME_R 1
#define HAVE_INET_NTOP 1
#define HAVE_INET_PTON 1
#define HAVE_LONGLONG 1
#define HAVE_MSG_NOSIGNAL 1
#define HAVE_PIPE 1
#define HAVE_POLL 1
#define HAVE_POLL_FINE 1
#define HAVE_RECV 1
#define HAVE_SA_FAMILY_T 1
#define HAVE_SELECT 1
#define HAVE_SEND 1
#define HAVE_SETLOCALE 1
#define HAVE_SIGACTION 1
#define HAVE_SIGNAL 1
#define HAVE_SNPRINTF 1
#define HAVE_SOCKADDR_IN6_SIN6_SCOPE_ID 1
#define HAVE_SOCKET 1
#define HAVE_SOCKETPAIR 1
#define HAVE_STRCASECMP 1
#define HAVE_STRDUP 1
#define HAVE_STRTOK_R 1
#define HAVE_STRTOLL 1
#define HAVE_STRUCT_SOCKADDR_STORAGE 1
#define HAVE_STRUCT_TIMEVAL 1
#define HAVE_UTIME 1
#define HAVE_UTIMES 0
#define HAVE_VARIADIC_MACROS_C99 1

#define SIZEOF_INT 4
#define SIZEOF_LONG 8
#define SIZEOF_LONG_LONG 8
#define SIZEOF_OFF_T 8
#define SIZEOF_CURL_OFF_T 8
#define SIZEOF_CURL_SOCKET_T 4
#define SIZEOF_SIZE_T 8
#define SIZEOF_TIME_T 8
#define STDC_HEADERS 1
#define _FILE_OFFSET_BITS 64

#endif
