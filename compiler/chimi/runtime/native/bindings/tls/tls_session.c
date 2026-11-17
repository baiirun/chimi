#include "tls_session.h"

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "mbedtls/error.h"
#include "mbedtls/platform_util.h"
#include "psa/crypto.h"

static int crypto_ready(void) {
  /* psa_crypto_init is idempotent and does its own locking when threading
     is configured; calling it per session keeps this file free of state. */
  return psa_crypto_init() == PSA_SUCCESS;
}

static void describe(int error, char *message, size_t size, const char *what) {
  char words[96];
  mbedtls_strerror(error, words, sizeof words);
  snprintf(message, size, "%s: %s", what, words);
}

/* PEM parsing wants the terminating NUL inside the length. */
static unsigned char *terminated(const unsigned char *bytes, size_t length) {
  unsigned char *copy = malloc(length + 1);
  if (copy) {
    memcpy(copy, bytes, length);
    copy[length] = 0;
  }
  return copy;
}

static int parse_certificates(mbedtls_x509_crt *chain, const unsigned char *pem, size_t length) {
  unsigned char *copy = terminated(pem, length);
  if (!copy) return MBEDTLS_ERR_X509_ALLOC_FAILED;
  int result = mbedtls_x509_crt_parse(chain, copy, length + 1);
  free(copy);
  return result;
}

static int buffer_append(TlsBuffer *buffer, const unsigned char *bytes, size_t length) {
  if (buffer->start > 0 && buffer->start == buffer->length) {
    buffer->start = 0;
    buffer->length = 0;
  }
  if (length > buffer->capacity - buffer->length) {
    if (buffer->start > 0) {
      memmove(buffer->bytes, buffer->bytes + buffer->start, buffer->length - buffer->start);
      buffer->length -= buffer->start;
      buffer->start = 0;
    }
    if (length > buffer->capacity - buffer->length) {
      size_t capacity = buffer->capacity ? buffer->capacity : 4096;
      while (capacity - buffer->length < length) capacity *= 2;
      unsigned char *grown = realloc(buffer->bytes, capacity);
      if (!grown) return -1;
      buffer->bytes = grown;
      buffer->capacity = capacity;
    }
  }
  memcpy(buffer->bytes + buffer->length, bytes, length);
  buffer->length += length;
  return 0;
}

static int send_to_buffer(void *context, const unsigned char *bytes, size_t length) {
  TlsSession *session = context;
  if (buffer_append(&session->outgoing, bytes, length) != 0) return MBEDTLS_ERR_SSL_ALLOC_FAILED;
  return (int)length;
}

static int receive_from_buffer(void *context, unsigned char *bytes, size_t length) {
  TlsSession *session = context;
  TlsBuffer *incoming = &session->incoming;
  size_t available = incoming->length - incoming->start;
  if (available == 0) return MBEDTLS_ERR_SSL_WANT_READ;
  if (length > available) length = available;
  memcpy(bytes, incoming->bytes + incoming->start, length);
  incoming->start += length;
  return (int)length;
}

/* Sessions made and not yet freed, for tests that check nothing leaks. */
static atomic_long live_sessions;

long tls_live_sessions(void) { return atomic_load(&live_sessions); }

static TlsSession *new_session(int endpoint, int *code, char *message, size_t size) {
  if (!crypto_ready()) {
    *code = TLS_INVALID;
    snprintf(message, size, "the crypto library failed to start");
    return NULL;
  }
  TlsSession *session = calloc(1, sizeof *session);
  if (!session) {
    *code = TLS_INVALID;
    snprintf(message, size, "out of memory");
    return NULL;
  }
  atomic_fetch_add(&live_sessions, 1);
  mbedtls_ssl_init(&session->ssl);
  mbedtls_ssl_config_init(&session->config);
  mbedtls_x509_crt_init(&session->roots);
  mbedtls_x509_crt_init(&session->certificate);
  mbedtls_pk_init(&session->key);
  int result = mbedtls_ssl_config_defaults(&session->config, endpoint, MBEDTLS_SSL_TRANSPORT_STREAM,
                                           MBEDTLS_SSL_PRESET_DEFAULT);
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the TLS settings were refused");
    tls_free_session(session);
    return NULL;
  }
  mbedtls_ssl_conf_min_tls_version(&session->config, MBEDTLS_SSL_VERSION_TLS1_2);
  return session;
}

/* Called once the config is complete. */
static TlsSession *start(TlsSession *session, int *code, char *message, size_t size) {
  int result = mbedtls_ssl_setup(&session->ssl, &session->config);
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the TLS session could not start");
    tls_free_session(session);
    return NULL;
  }
  mbedtls_ssl_set_bio(&session->ssl, session, send_to_buffer, receive_from_buffer, NULL);
  return session;
}

TlsSession *tls_client_session(const unsigned char *roots, size_t roots_length,
                               const char *host, int *code, char *message, size_t size) {
  TlsSession *session = new_session(MBEDTLS_SSL_IS_CLIENT, code, message, size);
  if (!session) return NULL;
  if (!roots) {
    if (tls_system_roots(&session->roots) != 0) {
      *code = TLS_NO_ROOTS;
      snprintf(message, size, "no trusted roots were found on this system");
      tls_free_session(session);
      return NULL;
    }
  }
  int result = roots ? parse_certificates(&session->roots, roots, roots_length) : 0;
  if (result < 0 || session->roots.raw.len == 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the trusted roots are not valid PEM certificates");
    tls_free_session(session);
    return NULL;
  }
  mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_REQUIRED);
  mbedtls_ssl_conf_ca_chain(&session->config, &session->roots, NULL);
  session = start(session, code, message, size);
  if (!session) return NULL;
  result = mbedtls_ssl_set_hostname(&session->ssl, host);
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the host name is not usable");
    tls_free_session(session);
    return NULL;
  }
  return session;
}

TlsSession *tls_server_session(const unsigned char *certificate, size_t certificate_length,
                               const unsigned char *key, size_t key_length, int *code,
                               char *message, size_t size) {
  TlsSession *session = new_session(MBEDTLS_SSL_IS_SERVER, code, message, size);
  if (!session) return NULL;
  int result = parse_certificates(&session->certificate, certificate, certificate_length);
  if (result < 0 || session->certificate.raw.len == 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the certificate is not valid PEM");
    tls_free_session(session);
    return NULL;
  }
  unsigned char *key_copy = terminated(key, key_length);
  result = key_copy ? mbedtls_pk_parse_key(&session->key, key_copy, key_length + 1, NULL, 0)
                    : MBEDTLS_ERR_PK_ALLOC_FAILED;
  if (key_copy) {
    mbedtls_platform_zeroize(key_copy, key_length);
    free(key_copy);
  }
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the private key is not a valid unencrypted PEM key");
    tls_free_session(session);
    return NULL;
  }
  result = mbedtls_pk_check_pair(&session->certificate.pk, &session->key);
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the private key is not the certificate's");
    tls_free_session(session);
    return NULL;
  }
  result = mbedtls_ssl_conf_own_cert(&session->config, &session->certificate, &session->key);
  if (result != 0) {
    *code = TLS_INVALID;
    describe(result, message, size, "the certificate and key do not form an identity");
    tls_free_session(session);
    return NULL;
  }
  mbedtls_ssl_conf_authmode(&session->config, MBEDTLS_SSL_VERIFY_NONE);
  return start(session, code, message, size);
}

void tls_free_session(TlsSession *session) {
  if (!session) return;
  atomic_fetch_sub(&live_sessions, 1);
  mbedtls_ssl_free(&session->ssl);
  mbedtls_ssl_config_free(&session->config);
  mbedtls_x509_crt_free(&session->roots);
  mbedtls_x509_crt_free(&session->certificate);
  mbedtls_pk_free(&session->key);
  free(session->incoming.bytes);
  free(session->outgoing.bytes);
  free(session);
}

static int fail(TlsSession *session, int error) {
  if (session->failure) return -1;
  uint32_t flags = 0;
  if (error == MBEDTLS_ERR_X509_CERT_VERIFY_FAILED) {
    flags = mbedtls_ssl_get_verify_result(&session->ssl);
  }
  /* An untrusted chain says nothing about its dates or names, so it is
     reported first: an attacker's expired certificate is Untrusted. */
  if (flags & MBEDTLS_X509_BADCERT_NOT_TRUSTED) {
    session->failure = TLS_UNTRUSTED;
    snprintf(session->message, sizeof session->message,
             "the peer's certificate is not signed by a trusted root");
  } else if (flags & (MBEDTLS_X509_BADCERT_EXPIRED | MBEDTLS_X509_BADCERT_FUTURE)) {
    session->failure = TLS_EXPIRED;
    snprintf(session->message, sizeof session->message,
             "the peer's certificate is expired or not yet valid");
  } else if (flags & MBEDTLS_X509_BADCERT_CN_MISMATCH) {
    session->failure = TLS_WRONG_HOST;
    snprintf(session->message, sizeof session->message,
             "the peer's certificate is for another host");
  } else if (flags != 0) {
    session->failure = TLS_UNTRUSTED;
    snprintf(session->message, sizeof session->message,
             "the peer's certificate is not acceptable (flags 0x%x)", (unsigned)flags);
  } else if (error == MBEDTLS_ERR_SSL_BAD_PROTOCOL_VERSION) {
    session->failure = TLS_UNACCEPTABLE;
    describe(error, session->message, sizeof session->message,
             "the peer offers no TLS version both sides accept");
  } else if (error == MBEDTLS_ERR_SSL_ALLOC_FAILED) {
    session->failure = TLS_INVALID;
    snprintf(session->message, sizeof session->message, "out of memory");
  } else {
    session->failure = TLS_HANDSHAKE;
    describe(error, session->message, sizeof session->message, "the TLS exchange failed");
  }
  return -1;
}

int tls_handshake(TlsSession *session) {
  if (session->failure) return -1;
  int result = mbedtls_ssl_handshake(&session->ssl);
  if (result == 0) return 1;
  if (result == MBEDTLS_ERR_SSL_WANT_READ) return 0;
  return fail(session, result);
}

int tls_feed(TlsSession *session, const unsigned char *bytes, size_t length) {
  return buffer_append(&session->incoming, bytes, length);
}

const unsigned char *tls_pending(const TlsSession *session, size_t *length) {
  *length = session->outgoing.length - session->outgoing.start;
  return session->outgoing.bytes + session->outgoing.start;
}

void tls_took(TlsSession *session) {
  session->outgoing.start = 0;
  session->outgoing.length = 0;
}

int tls_send(TlsSession *session, const unsigned char *bytes, size_t length) {
  if (session->failure) return -1;
  size_t sent = 0;
  while (sent < length) {
    int result = mbedtls_ssl_write(&session->ssl, bytes + sent, length - sent);
    if (result < 0) return fail(session, result);
    sent += (size_t)result;
  }
  return 0;
}

int tls_receive(TlsSession *session, unsigned char *buffer, size_t size, size_t *length) {
  if (session->failure) return -1;
  /* mbedTLS answers WANT_READ after a record that carried no data (a session
     ticket, a warning alert) even when later records are already fed in,
     and 0 for an empty data record, which is not the end (this transport
     never reports one). So it reads on while anything is unread, here or in
     mbedTLS, and needs input only once a call finds nothing. A call that
     consumes nothing with input waiting is retried a few times, so a stuck
     state cannot spin. */
  int idle = 0;
  for (;;) {
    size_t consumed = session->incoming.start;
    int result = mbedtls_ssl_read(&session->ssl, buffer, size);
    if (result > 0) {
      *length = (size_t)result;
      return TLS_DATA;
    }
    if (result == MBEDTLS_ERR_SSL_PEER_CLOSE_NOTIFY) return TLS_PEER_CLOSED;
    if (result != 0 && result != MBEDTLS_ERR_SSL_WANT_READ &&
        result != MBEDTLS_ERR_SSL_RECEIVED_NEW_SESSION_TICKET) {
      return fail(session, result);
    }
    int unread = session->incoming.start < session->incoming.length ||
                 mbedtls_ssl_check_pending(&session->ssl);
    idle = session->incoming.start != consumed ? 0 : idle + 1;
    if (!unread || idle > 4) return TLS_NEEDS_INPUT;
  }
}

void tls_close_notify(TlsSession *session) {
  if (!session->failure) (void)mbedtls_ssl_close_notify(&session->ssl);
}

int tls_check_roots(const unsigned char *pem, size_t length, char *message, size_t size) {
  if (!crypto_ready()) {
    snprintf(message, size, "the crypto library failed to start");
    return -1;
  }
  mbedtls_x509_crt chain;
  mbedtls_x509_crt_init(&chain);
  int result = parse_certificates(&chain, pem, length);
  int count = 0;
  if (result >= 0) {
    for (mbedtls_x509_crt *each = &chain; each && each->raw.len; each = each->next) count++;
  }
  mbedtls_x509_crt_free(&chain);
  if (result < 0 || count == 0) {
    describe(result, message, size, "the roots are not valid PEM certificates");
    return -1;
  }
  return count;
}

/* Debian and Ubuntu, Fedora and RHEL, macOS and Alpine, openSUSE. */
static const char *const BUNDLES[] = {
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
};

/* A bundle may hold a certificate mbedTLS cannot parse (a positive result
   counts those); it loads as long as one root parsed. */
static int loaded(mbedtls_x509_crt *chain, const char *path) {
  int result = mbedtls_x509_crt_parse_file(chain, path);
  return result >= 0 && chain->raw.len > 0;
}

int tls_system_roots(mbedtls_x509_crt *chain) {
  const char *named = getenv("SSL_CERT_FILE");
  if (named && *named) return loaded(chain, named) ? 0 : -1;
  for (size_t index = 0; index < sizeof BUNDLES / sizeof BUNDLES[0]; index++) {
    if (loaded(chain, BUNDLES[index])) return 0;
    mbedtls_x509_crt_free(chain);
    mbedtls_x509_crt_init(chain);
  }
  return -1;
}
