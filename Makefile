# SPDX-License-Identifier: GPL-3.0-or-later
CC ?= cc
CFLAGS ?= -O2 -g -Wall -Wextra -Werror -std=gnu11

PKG_CONFIG ?= pkg-config
LIBUSB_CFLAGS := $(shell $(PKG_CONFIG) --cflags libusb-1.0 2>/dev/null)
LIBUSB_LIBS := $(shell $(PKG_CONFIG) --libs libusb-1.0 2>/dev/null)

ifeq ($(strip $(LIBUSB_LIBS)),)
# Fallback for systems with the runtime library but no -devel package.
CPPFLAGS += -Ithird_party/libusb
LDLIBS += -l:libusb-1.0.so.0
else
CPPFLAGS += $(LIBUSB_CFLAGS)
LDLIBS += $(LIBUSB_LIBS)
endif

LDLIBS += -lm

all: rme-probe

# The vendored libusb header is optional; pkg-config's system libusb is the
# default.  The hwdep backend includes the driver's uapi header.
VENDORED_LIBUSB := $(wildcard third_party/libusb/libusb.h)

rme-probe: tools/rme-probe.c kernel/ffuc-uapi.h $(VENDORED_LIBUSB)
	$(CC) $(CPPFLAGS) $(CFLAGS) -o $@ tools/rme-probe.c $(LDFLAGS) $(LDLIBS)

clean:
	rm -f rme-probe

.PHONY: all clean
