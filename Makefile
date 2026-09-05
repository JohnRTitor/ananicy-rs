NAME := ananicy-rs

PREFIX ?= /usr
BINDIR ?= $(PREFIX)/bin
LIBDIR ?= $(PREFIX)/lib
SYSTEMD_SYSTEM_UNIT_DIR ?= $(LIBDIR)/systemd/system

CARGO_TARGET_DIR ?= target
DEBUG ?= 0
ifeq ($(DEBUG),0)
	TARGET := release
	PROFILE_ARGS := --release
else
	TARGET := debug
	PROFILE_ARGS :=
endif

BIN_SRC := $(CARGO_TARGET_DIR)/$(TARGET)/$(NAME)
BIN_DST := $(DESTDIR)$(BINDIR)/$(NAME)

.PHONY: all build clean install

all: build

build:
	cargo build $(PROFILE_ARGS)

clean:
	cargo clean

install:
	install -Dm0755 $(BIN_SRC) $(BIN_DST)
	
	install -dm0755 $(DESTDIR)$(SYSTEMD_SYSTEM_UNIT_DIR)
	sed -e 's|@bindir@|$(BINDIR)|g' data/$(NAME).service.in > data/$(NAME).service
	install -Dm0644 data/$(NAME).service $(DESTDIR)$(SYSTEMD_SYSTEM_UNIT_DIR)/$(NAME).service
