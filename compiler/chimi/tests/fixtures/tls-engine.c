/* The std/tls session engine (bindings/tls/tls_session.c) against records a
   Tls.server never sends: empty data records, and many session tickets in
   one record. Each scenario feeds the client everything at once, then reads:
   it must read "hello" and then the close, never ask for input it already
   holds. Prints one line per scenario; tests/tls.rs compares them.
   Usage: tls-engine <fixture directory> */
#define MBEDTLS_ALLOW_PRIVATE_ACCESS
#include "tls_session.h"
#include "ssl_misc.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned char *slurp(const char *directory, const char *name, size_t *length) {
  char path[1024];
  snprintf(path, sizeof path, "%s/%s", directory, name);
  FILE *file = fopen(path, "rb");
  if (!file) {
    perror(path);
    exit(2);
  }
  fseek(file, 0, SEEK_END);
  *length = (size_t)ftell(file);
  fseek(file, 0, SEEK_SET);
  unsigned char *bytes = malloc(*length);
  if (fread(bytes, 1, *length, file) != *length) exit(2);
  fclose(file);
  return bytes;
}

static void move(TlsSession *from, TlsSession *to) {
  size_t length;
  const unsigned char *pending = tls_pending(from, &length);
  if (length) tls_feed(to, pending, length);
  tls_took(from);
}

/* `empties` empty data records, or `tickets` NewSessionTickets in one
   record, then "hello" and close_notify. */
static void scenario(const char *fixtures, const char *name, int empties, int tickets) {
  size_t certificate_length, key_length, roots_length;
  unsigned char *certificate = slurp(fixtures, "server.pem", &certificate_length);
  unsigned char *key = slurp(fixtures, "server.key", &key_length);
  unsigned char *roots = slurp(fixtures, "ca.pem", &roots_length);
  int code;
  char message[256];
  TlsSession *server = tls_server_session(certificate, certificate_length, key, key_length,
                                          &code, message, sizeof message);
  TlsSession *client = tls_client_session(roots, roots_length, "localhost", &code, message,
                                          sizeof message);
  if (!server || !client) {
    printf("%s: setup failed: %s\n", name, message);
    exit(1);
  }
  mbedtls_ssl_conf_new_session_tickets(&server->config, 0);
  int client_done = 0, server_done = 0;
  for (int round = 0; round < 50 && !(client_done && server_done); round++) {
    if (!client_done && (client_done = tls_handshake(client)) < 0) exit(1);
    move(client, server);
    if (!server_done && (server_done = tls_handshake(server)) < 0) exit(1);
    move(server, client);
  }
  for (int index = 0; index < empties; index++) {
    if (mbedtls_ssl_write(&server->ssl, (const unsigned char *)"", 0) < 0) exit(1);
  }
  if (tickets > 0) {
    unsigned char *out = server->ssl.out_msg;
    size_t length = 0;
    for (int index = 0; index < tickets; index++) {
      unsigned char ticket[] = {4, 0, 0, 18, 0, 0, 0x0e, 0x10, 0, 0, 0, 0, 1,
                                (unsigned char)index, 0, 4, 'a', 'b', 'c', 'd', 0, 0};
      memcpy(out + length, ticket, sizeof ticket);
      length += sizeof ticket;
    }
    server->ssl.out_msgtype = MBEDTLS_SSL_MSG_HANDSHAKE;
    server->ssl.out_msglen = length;
    if (mbedtls_ssl_write_record(&server->ssl, 1) != 0) exit(1);
  }
  if (tls_send(server, (const unsigned char *)"hello", 5) != 0) exit(1);
  tls_close_notify(server);
  move(server, client);
  unsigned char buffer[16384];
  size_t got = 0;
  printf("%s:", name);
  for (int read = 0; read < 4; read++) {
    int status = tls_receive(client, buffer, sizeof buffer, &got);
    if (status == TLS_DATA) {
      printf(" data %.*s", (int)got, buffer);
    } else if (status == TLS_PEER_CLOSED) {
      printf(" closed");
      break;
    } else if (status == TLS_NEEDS_INPUT) {
      printf(" needs-input");
      break;
    } else {
      printf(" failed %s", client->message);
      break;
    }
  }
  printf("\n");
  tls_free_session(client);
  tls_free_session(server);
  free(certificate);
  free(key);
  free(roots);
}

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  scenario(argv[1], "one empty record", 1, 0);
  scenario(argv[1], "two empty records", 2, 0);
  scenario(argv[1], "three empty records", 3, 0);
  scenario(argv[1], "two tickets in a record", 0, 2);
  scenario(argv[1], "eight tickets in a record", 0, 8);
  scenario(argv[1], "forty tickets in a record", 0, 40);
  return 0;
}
