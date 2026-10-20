-- Minimal Hyprland 0.56 (Lua) config for the headless w3bs rig.
hl.monitor({ output = "", mode = "1920x1080@60", position = "0x0", scale = 1 })
hl.config({
    general    = { gaps_in = 0, gaps_out = 0, border_size = 1, layout = "dwindle" },
    decoration = { rounding = 0, shadow = { enabled = false }, blur = { enabled = false } },
    animations = { enabled = false },
    misc       = { force_default_wallpaper = 0, disable_hyprland_logo = true, disable_splash_rendering = true },
    input      = { kb_layout = "us", follow_mouse = 1 },
})
