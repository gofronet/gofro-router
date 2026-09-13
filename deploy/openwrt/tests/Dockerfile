FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends iproute2 nftables python3 conntrack dnsmasq-base jq git cmake gcc make libc6-dev libjson-c-dev pkg-config ca-certificates
# Native library/tool revisions from OpenWrt's openwrt-25.12 package definitions.
RUN git clone https://github.com/openwrt/libubox.git /build/libubox \
    && git -C /build/libubox checkout 7dd127841e82eb1cfb61185da37dde7b9bd9ba6d \
    && cmake -S /build/libubox -B /build/libubox/out -DBUILD_LUA=OFF -DBUILD_EXAMPLES=OFF \
    && cmake --build /build/libubox/out && cmake --install /build/libubox/out \
    && git clone https://github.com/openwrt/jsonpath.git /build/jsonpath \
    && git -C /build/jsonpath checkout b9034210bd331749673416c6bf389cccd4e23610 \
    && cmake -S /build/jsonpath -B /build/jsonpath \
    && cmake --build /build/jsonpath && cmake --install /build/jsonpath \
    && git clone https://github.com/openwrt/uci.git /build/uci \
    && git -C /build/uci checkout 66127cd76c5d0bd46d5a90302cc6110f53a4e2f8 \
    && cmake -S /build/uci -B /build/uci/out -DBUILD_LUA=OFF \
    && cmake --build /build/uci/out && cmake --install /build/uci/out \
    && ln -s /usr/local/bin/jsonpath /usr/local/bin/jsonfilter \
    && ldconfig && rm -rf /build /var/lib/apt/lists/*
