// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _ARPA_NAMESER_H
#define _ARPA_NAMESER_H

#define NS_PACKETSZ 512
#define NS_MAXDNAME 1025
#define NS_HFIXEDSZ 12
#define NS_QFIXEDSZ 4
#define NS_RRFIXEDSZ 10
#define PACKETSZ NS_PACKETSZ
#define MAXDNAME NS_MAXDNAME
#define HFIXEDSZ NS_HFIXEDSZ
#define QFIXEDSZ NS_QFIXEDSZ
#define RRFIXEDSZ NS_RRFIXEDSZ

#define ns_c_in 1
#define C_IN 1
#define ns_t_a 1
#define ns_t_ns 2
#define ns_t_cname 5
#define ns_t_soa 6
#define ns_t_ptr 12
#define ns_t_mx 15
#define ns_t_txt 16
#define ns_t_aaaa 28
#define ns_t_srv 33
#define ns_t_any 255
#define T_A ns_t_a
#define T_NS ns_t_ns
#define T_CNAME ns_t_cname
#define T_SOA ns_t_soa
#define T_PTR ns_t_ptr
#define T_MX ns_t_mx
#define T_TXT ns_t_txt
#define T_AAAA ns_t_aaaa
#define T_SRV ns_t_srv
#define T_ANY ns_t_any

#endif
