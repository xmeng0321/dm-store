# SPDX-License-Identifier: GPL-2.0-only
#
# Copyright (C) 2024 OpenWrt.org

include $(TOPDIR)/rules.mk

PKG_NAME:=dm-store
PKG_VERSION:=0.1.0
PKG_RELEASE:=1

PKG_SOURCE_PROTO:=git
PKG_SOURCE_URL:=https://github.com/xmeng0321/dm-store.git
PKG_SOURCE_VERSION:=main
PKG_MIRROR_HASH:=skip

PKG_LICENSE:=MIT
PKG_LICENSE_FILES:=LICENSE

PKG_BUILD_DEPENDS:=rust/host

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

define Package/dm-store
  SECTION:=utils
  CATEGORY:=Utilities
  TITLE:=TR-181 data model store and manager
  DEPENDS:=$(RUST_ARCH_DEPENDS)
  URL:=https://github.com/xmeng0321/dm-store
endef

define Package/dm-store/description
  A TR-181 data model storage and management system written in Rust.
  Provides persistent SQLite-backed storage with hash-accelerated lookups,
  schema-aware validation, and transactional session support for CPE
  device data models. Includes both dm-store (storage engine CLI) and
  dm-manager (schema-aware manager CLI) binaries.
endef

define Build/Compile
	$(call Build/Compile/Cargo,, \
		--release \
		--workspace)
endef

define Package/dm-store/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/dm-store $(1)/usr/bin/dm-store
	$(INSTALL_BIN) $(PKG_INSTALL_DIR)/bin/dm-manager $(1)/usr/bin/dm-manager
endef

$(eval $(call BuildPackage,dm-store))
