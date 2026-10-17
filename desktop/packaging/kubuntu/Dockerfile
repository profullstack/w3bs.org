# syntax=docker/dockerfile:1
# A headless Kubuntu desktop for AI agents, watched from w3bs.org/fleet.
#
# KWin (Wayland) drives a virtual display: the host's vkms device, a KMS display with no
# hardware behind it, so Mesa composites with OpenGL on the CPU (llvmpipe) and no GPU is
# needed. Plasma, Konsole and the Agentic Desktop Layer (`w3bs-desktop`, Enabled from the
# start, MCP on 127.0.0.1:7801) run on that KWin. TigerVNC's w0vncserver shares the session
# through the KDE portal on a Unix socket in /vnc, which the host's `w3bs fleet agent` relays
# to w3bs.org; no TCP port is opened.
#
# Built and run by `w3bs fleet desktop start`; see w3bs.org/docs/fleet-screen.md.

# Ubuntu packages TigerVNC 1.15, which has no Wayland server; w0vncserver arrived in 1.16.
FROM ubuntu:26.04 AS w0vncserver
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake pkg-config ca-certificates curl \
      zlib1g-dev libpixman-1-dev libjpeg-turbo8-dev libglib2.0-dev libpipewire-0.3-dev uuid-dev \
      libwayland-dev libwayland-bin wayland-protocols libxkbcommon-dev libpam0g-dev \
      libx11-dev libxext-dev libxtst-dev libxdamage-dev libxfixes-dev libxrandr-dev libxi-dev libxcursor-dev
ARG TIGERVNC_VERSION=1.16.2
ARG TIGERVNC_SHA256=b107c0c8b8a962594281690366c24186e95c2ea4a169acbc0076aa62ed01f467
RUN curl -fsSL -o /tmp/tigervnc.tar.gz "https://github.com/TigerVNC/tigervnc/archive/refs/tags/v${TIGERVNC_VERSION}.tar.gz" \
    && echo "${TIGERVNC_SHA256}  /tmp/tigervnc.tar.gz" | sha256sum -c - \
    && mkdir /src && tar xzf /tmp/tigervnc.tar.gz -C /src --strip-components=1
WORKDIR /build
RUN cmake /src -DCMAKE_BUILD_TYPE=Release -DBUILD_VIEWER=OFF -DENABLE_NLS=OFF -DENABLE_GNUTLS=OFF \
      -DENABLE_NETTLE=OFF -DENABLE_H264=OFF -DENABLE_WAYLAND=ON -DINSTALL_SYSTEMD_UNITS=OFF \
    && make -j"$(nproc)" w0vncserver && install -m 0755 unix/w0vncserver/w0vncserver /usr/local/bin/

FROM ubuntu:26.04
ARG DESKTOP_VERSION=0.4.5
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8
RUN apt-get update && apt-get install -y --no-install-recommends \
      kwin-wayland plasma-desktop plasma-workspace konsole breeze breeze-cursor-theme libkscreen-bin \
      xdg-desktop-portal xdg-desktop-portal-kde pipewire wireplumber dbus-daemon flatpak \
      libegl-mesa0 libgl1-mesa-dri libpipewire-0.3-0t64 libjpeg-turbo8 libpixman-1-0 libpam0g \
      fonts-noto-core fonts-noto-cjk fonts-noto-color-emoji ca-certificates curl gnupg tini procps \
    && rm -rf /var/lib/apt/lists/*
# A browser for agents: Google Chrome from Google's signed repository (Ubuntu's Chromium is a snap,
# and snaps do not run in a container).
RUN install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /etc/apt/keyrings/google-chrome.gpg \
    && echo "deb [arch=amd64 signed-by=/etc/apt/keyrings/google-chrome.gpg] https://dl.google.com/linux/chrome/deb/ stable main" \
       > /etc/apt/sources.list.d/google-chrome.list \
    && apt-get update && apt-get install -y --no-install-recommends google-chrome-stable \
    && rm -rf /var/lib/apt/lists/*
COPY browser.sh /usr/local/bin/w3bs-browser
# KWin asks for real-time scheduling, which a container's capability set does not grant.
RUN setcap -r /usr/bin/kwin_wayland 2>/dev/null || true
COPY --from=w0vncserver /usr/local/bin/w0vncserver /usr/local/bin/w0vncserver
RUN arch="$(uname -m)" \
    && base="https://github.com/profullstack/w3bs.org/releases/download/desktop-v${DESKTOP_VERSION}" \
    && file="w3bs-desktop-${DESKTOP_VERSION}-${arch}-unknown-linux-gnu.tar.gz" \
    && cd /tmp && curl -fsSLO "$base/$file" && curl -fsSLO "$base/SHA256SUMS" \
    && grep " $file\$" SHA256SUMS | sha256sum -c - \
    && tar xzf "$file" ./bin/w3bs-desktop && install -m 0755 bin/w3bs-desktop /usr/local/bin/ \
    && rm -rf /tmp/*
COPY session.sh /usr/local/bin/w3bs-kubuntu-session
RUN chmod 0755 /usr/local/bin/w3bs-kubuntu-session /usr/local/bin/w3bs-browser && mkdir -p /vnc /home/desktop && chmod 1777 /vnc /home/desktop
ENV HOME=/home/desktop
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/w3bs-kubuntu-session"]
