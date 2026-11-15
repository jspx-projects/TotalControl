# SPDX-License-Identifier: GPL-2.0-only
#
# Out-of-tree build of the RME Fireface USB transport driver.
#
#   make                 build against the running kernel
#   make KDIR=/path      build against another kernel tree
#   sudo make install    install into /lib/modules/$(uname -r)/updates
#   make test            host tests of the module's pure logic (no kernel)
#
# This module is a transport core: USB, PCM and the snd-hwdep raw control
# node.  Mixer/device policy lives in TotalControl (userspace).

snd-fireface-uc-y := ffuc.o ffuc-hwdep.o ffuc-midi.o
obj-m += snd-fireface-uc.o

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) modules

install: all
	$(MAKE) -C $(KDIR) M=$(CURDIR) INSTALL_MOD_DIR=updates modules_install
	depmod -a $(KVER)

test:
	$(CC) -std=c11 -Wall -Wextra -Werror -o test/meter-pair-test test/meter-pair-test.c
	./test/meter-pair-test

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) clean
	$(RM) test/meter-pair-test

.PHONY: all install test clean
