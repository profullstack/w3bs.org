# syntax=docker/dockerfile:1
# W3BS OS Docker editions (docs/os-prd.md §20), on Alpine Linux.
#
#   docker build -f os/docker/Dockerfile --target server  -t w3bs-os:server  .
#   docker build -f os/docker/Dockerfile --target desktop -t w3bs-os:desktop .
#
# Build from the repository root: the w3bs CLI and the Agentic Desktop Layer are compiled
# here for musl, since their published release binaries are glibc builds.
ARG ALPINE=alpine:3.24

# --- w3bs CLI and w3bs-desktop, native musl builds -------------------------------------
FROM ${ALPINE} AS rust
RUN apk add --no-cache rust cargo build-base cmake perl pkgconf linux-headers
WORKDIR /src
COPY package-manager/ package-manager/
COPY desktop/ desktop/
COPY fixtures/ fixtures/
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/src/package-manager/target \
    --mount=type=cache,target=/src/desktop/target \
    cd package-manager && cargo build --locked --release --bin w3bs \
    && install -m 0755 target/release/w3bs /usr/local/bin/w3bs \
    && cd ../desktop && cargo build --locked --release -p w3bs-desktop \
    && install -m 0755 target/release/w3bs-desktop /usr/local/bin/w3bs-desktop

# --- TigerVNC's Wayland server (Alpine packages TigerVNC without it) -------------------
FROM ${ALPINE} AS w0vncserver
RUN apk add --no-cache build-base cmake pkgconf curl zlib-dev pixman-dev libjpeg-turbo-dev \
      glib-dev pipewire-dev util-linux-dev wayland-dev wayland-protocols libxkbcommon-dev \
      linux-pam-dev libx11-dev libxext-dev libxtst-dev libxdamage-dev libxfixes-dev \
      libxrandr-dev libxi-dev libxcursor-dev
ARG TIGERVNC_VERSION=1.16.2
ARG TIGERVNC_SHA256=b107c0c8b8a962594281690366c24186e95c2ea4a169acbc0076aa62ed01f467
RUN curl -fsSL -o /tmp/tigervnc.tar.gz "https://github.com/TigerVNC/tigervnc/archive/refs/tags/v${TIGERVNC_VERSION}.tar.gz" \
    && echo "${TIGERVNC_SHA256}  /tmp/tigervnc.tar.gz" | sha256sum -c - \
    && mkdir /src && tar xzf /tmp/tigervnc.tar.gz -C /src --strip-components=1
WORKDIR /build
RUN cmake /src -DCMAKE_BUILD_TYPE=Release -DBUILD_VIEWER=OFF -DENABLE_NLS=OFF -DENABLE_GNUTLS=OFF \
      -DENABLE_NETTLE=OFF -DENABLE_H264=OFF -DENABLE_WAYLAND=ON -DINSTALL_SYSTEMD_UNITS=OFF \
    && make -j"$(nproc)" w0vncserver vncpasswd \
    && install -m 0755 unix/w0vncserver/w0vncserver unix/vncpasswd/vncpasswd /usr/local/bin/

# --- server: zsh, the w3bs CLI, and what curl-installed suite tools need ----------------
FROM ${ALPINE} AS server
LABEL org.opencontainers.image.title="W3BS OS server" \
      org.opencontainers.image.description="Alpine Linux with zsh and the w3bs CLI (w3bs.org/os)" \
      org.opencontainers.image.source="https://github.com/profullstack/w3bs.org" \
      org.opencontainers.image.licenses="MIT"
RUN apk add --no-cache zsh zsh-vcs bash ca-certificates curl git openssh-client-default nodejs npm \
      tini tzdata procps less caddy \
    && adduser -D -u 1000 -s /bin/zsh w3bs
# Caddy is the W3BS OS web server (docs/os-prd.md): automatic HTTPS, IPv6 by default. The
# Caddyfile only imports /etc/caddy/sites/*, the same layout as the ISO install, so tools
# add a site by dropping one file there. Installed, not started: run `caddy run` yourself.
RUN mkdir -p /etc/caddy/sites \
    && printf 'import /etc/caddy/sites/*\n' > /etc/caddy/Caddyfile \
    && chown w3bs:w3bs /etc/caddy/sites
COPY --from=rust /usr/local/bin/w3bs /usr/local/bin/w3bs
COPY registry/FINGERPRINT /usr/share/w3bs-os/FINGERPRINT
COPY os/docker/init.sh /usr/local/bin/w3bs-os-init
COPY os/docker/zshrc /etc/zsh/zshrc.w3bs
# Alpine's /etc/profile resets PATH on login; tools installed with w3bs land in ~/.local/bin.
RUN printf 'export PATH="$HOME/.local/bin:$PATH"\n' > /etc/profile.d/w3bs.sh \
    && cat /etc/zsh/zshrc.w3bs >> /etc/zsh/zshrc && rm /etc/zsh/zshrc.w3bs \
    && chmod 0755 /usr/local/bin/w3bs-os-init /usr/share/w3bs-os \
    && chmod 0644 /usr/share/w3bs-os/FINGERPRINT
# /etc/w3bs-os-release: what `w3bs os status` reports and `w3bs os upgrade` keeps current.
ARG W3BS_OS_VERSION=0.1.0
RUN rel=$(cat /etc/alpine-release) \
    && printf 'ID=w3bs-os\nNAME="W3BS OS"\nVERSION=%s\nEDITION=docker-server\nALPINE=%s\nALPINE_BRANCH=v%s\nHOME_URL=https://w3bs.org/os\n' \
       "$W3BS_OS_VERSION" "$rel" "$(echo "$rel" | cut -d. -f1,2)" > /etc/w3bs-os-release
ENV LANG=C.UTF-8 SHELL=/bin/zsh PATH=/home/w3bs/.local/bin:$PATH
USER w3bs
WORKDIR /home/w3bs
ENTRYPOINT ["/sbin/tini", "--", "/usr/local/bin/w3bs-os-init"]
CMD ["/bin/zsh", "-l"]

# --- desktop: server + KDE Plasma on a virtual display + the Agentic Desktop Layer -----
FROM server AS desktop
LABEL org.opencontainers.image.title="W3BS OS desktop" \
      org.opencontainers.image.description="Alpine Linux KDE Plasma desktop with the Agentic Desktop Layer (w3bs.org/os)"
USER root
RUN apk add --no-cache kwin plasma-desktop plasma-workspace konsole breeze breeze-cursors \
      libkscreen kscreen xdg-desktop-portal xdg-desktop-portal-kde pipewire wireplumber dbus \
      dbus-x11 flatpak mesa-dri-gallium mesa-egl mesa-gbm font-noto font-noto-cjk font-noto-emoji \
      chromium libcap-setcap pixman libjpeg-turbo linux-pam \
    && (setcap -r /usr/bin/kwin_wayland 2>/dev/null || true) \
    && sed -i 's/^EDITION=.*/EDITION=docker-desktop/' /etc/w3bs-os-release
COPY --from=w0vncserver /usr/local/bin/w0vncserver /usr/local/bin/vncpasswd /usr/local/bin/
COPY --from=rust /usr/local/bin/w3bs-desktop /usr/local/bin/w3bs-desktop
COPY os/docker/session.sh /usr/local/bin/w3bs-os-session
COPY os/docker/browser.sh /usr/local/bin/w3bs-browser
RUN chmod 0755 /usr/local/bin/w3bs-os-session /usr/local/bin/w3bs-browser \
    && mkdir -p /vnc /home/desktop && chmod 1777 /vnc /home/desktop
ENV HOME=/home/desktop PATH=/home/desktop/.local/bin:$PATH XDG_RUNTIME_DIR=/tmp/runtime-w3bs
WORKDIR /home/desktop
USER w3bs
CMD ["/usr/local/bin/w3bs-os-session"]
