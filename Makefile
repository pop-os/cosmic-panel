prefix ?= /usr/local
bindir = $(prefix)/bin
libdir = $(prefix)/lib
includedir = $(prefix)/include
datarootdir = $(prefix)/share
datadir = $(datarootdir)

TARGET = debug
DEBUG ?= 0
ifeq ($(DEBUG),0)
	TARGET = release
	ARGS += --release
endif

VENDOR ?= 0
ifneq ($(VENDOR),0)
	ARGS += --frozen
endif

BIN = cosmic-panel

all: $(BIN) $(PKGCONFIG)

clean:
	rm -rf target

distclean: clean
	rm -rf .cargo vendor vendor.tar

$(BIN): Cargo.toml Cargo.lock ${BIN}-bin/src/main.rs vendor-check
	cargo build $(ARGS) --bin ${BIN}

install:
	install -Dm0755 target/$(TARGET)/$(BIN) $(DESTDIR)$(bindir)/$(BIN)

## Cargo Vendoring

vendor:
	rm .cargo -rf
	mkdir -p .cargo
	cargo vendor | head -n -1 > .cargo/config
	echo 'directory = "vendor"' >> .cargo/config
	tar cf vendor.tar vendor
	rm -rf vendor

vendor-check:
ifeq ($(VENDOR),1)
	rm vendor -rf && tar xf vendor.tar
endif
