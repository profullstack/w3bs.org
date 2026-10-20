# Headless Hyprland rig for agent automation tests (see hyprland.sh).
# wlrctl is AUR-only on Arch, so it is built from source in a throwaway stage.
FROM archlinux:latest AS wlrctl
RUN pacman -Syu --noconfirm --needed base-devel meson ninja git wayland wayland-protocols libxkbcommon scdoc \
 && git clone --depth 1 https://git.sr.ht/~brocellous/wlrctl /src \
 && cd /src && meson setup build --prefix=/usr && ninja -C build && DESTDIR=/out ninja -C build install

FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed \
      hyprland xdg-desktop-portal-hyprland grim wtype foot jq mesa seatd \
      dbus procps-ng file python ttf-dejavu \
 && pacman -Scc --noconfirm && rm -rf /var/cache/pacman/pkg/*
COPY --from=wlrctl /out/usr/ /usr/
COPY hyprland.lua /root/.config/hypr/hyprland.lua
COPY rig-session /usr/local/bin/rig-session
RUN chmod 0755 /usr/local/bin/rig-session
CMD ["/usr/local/bin/rig-session"]
