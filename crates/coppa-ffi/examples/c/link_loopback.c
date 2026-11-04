/*
 * COP-139: two coppa links in one process over an in-memory audio cable.
 *
 * N0BBB listens, N0AAA connects, sends 23 bytes and disconnects. Each tick is
 * 20 ms: both links get the other's previous 960-sample block (48 kHz mono)
 * and produce their next one. A real program calls coppa_link_process from
 * its audio callback instead and keys the transmitter while tx_active > 0.
 *
 * Exits 0 only if the bytes arrive intact and all four events are seen.
 * Builds as C11 and as C++17 with -Wall -Wextra -Wpedantic -Werror; CI runs
 * both (.github/workflows/ci.yml).
 */
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "coppa.h"

#define BLOCK 960
#define TICK_MS 20
#define MAX_TICKS 6000 /* 120 s of virtual time */

static const char *reason_name(int32_t reason) {
  switch (reason) {
  case COPPA_LINK_REASON_LOCAL:
    return "local";
  case COPPA_LINK_REASON_REMOTE:
    return "remote";
  case COPPA_LINK_REASON_TIMEOUT:
    return "timeout";
  case COPPA_LINK_REASON_RETRIES_EXHAUSTED:
    return "retries exhausted";
  case COPPA_LINK_REASON_HANDSHAKE_FAILED:
    return "handshake failed";
  default:
    return "none";
  }
}

static coppa_link_t *make_link(const char *callsign) {
  coppa_link_config_t cfg;
  coppa_link_t *link;
  memset(&cfg, 0, sizeof cfg);
  cfg.struct_size = (uint32_t)sizeof cfg;
  cfg.callsign = callsign;
  link = coppa_link_new(&cfg);
  if (link == NULL) {
    const char *err = coppa_last_error();
    fprintf(stderr, "coppa_link_new(%s) failed: %s\n", callsign, err ? err : "?");
  }
  return link;
}

/* Print and count the link's pending events. */
static void drain_events(coppa_link_t *link, const char *self, int *connected,
                         int *disconnected) {
  coppa_link_event_t ev;
  while (coppa_link_next_event(link, &ev) == COPPA_OK) {
    if (ev.kind == COPPA_LINK_EVENT_CONNECTED) {
      printf("%s connected to %s\n", self, ev.remote);
      *connected = 1;
    } else if (ev.kind == COPPA_LINK_EVENT_DISCONNECTED) {
      printf("%s disconnected (%s)\n", self, reason_name(ev.reason));
      *disconnected = 1;
    }
  }
}

int main(void) {
  static float a_out[BLOCK], b_out[BLOCK], a_next[BLOCK], b_next[BLOCK];
  static const char msg[] = "hello over a coppa link";
  const uintptr_t msg_len = sizeof msg - 1;
  uint8_t got[64];
  uintptr_t got_len = 0;
  int a_conn = 0, b_conn = 0, a_disc = 0, b_disc = 0, sent = 0, closing = 0;
  uint64_t tick;
  coppa_link_t *a = make_link("N0AAA");
  coppa_link_t *b = make_link("N0BBB");
  if (a == NULL || b == NULL)
    return 1;

  if (coppa_link_listen(b, true) != COPPA_OK || coppa_link_connect(a, "N0BBB", 0) != COPPA_OK) {
    fprintf(stderr, "listen/connect failed: %s\n", coppa_last_error());
    return 1;
  }

  for (tick = 0; tick < MAX_TICKS && !(a_disc && b_disc); tick++) {
    uint64_t now = tick * TICK_MS;
    uintptr_t n = 0;
    if (coppa_link_process(a, now, b_out, BLOCK, a_next, BLOCK, NULL) != COPPA_OK ||
        coppa_link_process(b, now, a_out, BLOCK, b_next, BLOCK, NULL) != COPPA_OK) {
      fprintf(stderr, "coppa_link_process failed: %s\n", coppa_last_error());
      return 1;
    }
    memcpy(a_out, a_next, sizeof a_out);
    memcpy(b_out, b_next, sizeof b_out);

    drain_events(a, "N0AAA", &a_conn, &a_disc);
    drain_events(b, "N0BBB", &b_conn, &b_disc);

    if (a_conn && b_conn && !sent) {
      if (coppa_link_send(a, (const uint8_t *)msg, msg_len) != COPPA_OK) {
        fprintf(stderr, "coppa_link_send failed: %s\n", coppa_last_error());
        return 1;
      }
      sent = 1;
    }

    if (coppa_link_recv(b, got + got_len, sizeof got - got_len, &n) != COPPA_OK)
      return 1;
    if (n > 0) {
      got_len += n;
      if (got_len == msg_len)
        printf("N0BBB received %u bytes: %.*s\n", (unsigned)got_len, (int)got_len,
               (const char *)got);
    }

    if (sent && !closing && got_len == msg_len) {
      uintptr_t pending = 1;
      if (coppa_link_pending(a, &pending) == COPPA_OK && pending == 0) {
        if (coppa_link_disconnect(a, now) != COPPA_OK) {
          fprintf(stderr, "coppa_link_disconnect failed: %s\n", coppa_last_error());
          return 1;
        }
        closing = 1;
      }
    }
  }

  coppa_link_destroy(&a);
  coppa_link_destroy(&b);

  if (got_len != msg_len || memcmp(got, msg, msg_len) != 0) {
    fprintf(stderr, "FAIL: N0BBB received %u of %u bytes\n", (unsigned)got_len,
            (unsigned)msg_len);
    return 1;
  }
  if (!(a_conn && b_conn && a_disc && b_disc)) {
    fprintf(stderr, "FAIL: missing events after %u ticks\n", (unsigned)tick);
    return 1;
  }
  return 0;
}
