# W3BS OS: tools installed with w3bs land in ~/.local/bin; add the W3BS repo at first login.
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac
[ -x /usr/share/w3bs-os/first-login.sh ] && /usr/share/w3bs-os/first-login.sh
