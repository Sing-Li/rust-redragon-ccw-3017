#!/bin/bash
set -e

if [ "$EUID" -eq 0 ]; then
  echo -e "\033[0;31mPlease run as a normal user (e.g., deck), not as root.\033[0m"
  exit 1
fi

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log(){ echo -e "$1$2$NC"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

if [[ "$1" == "uninstall" || "$1" == "-u" ]]; then
  log $YELLOW "Uninstalling Redragon LCD (SteamOS)"
  systemctl --user stop redragon-lcd 2>/dev/null || true
  systemctl --user disable redragon-lcd 2>/dev/null || true
  rm -f /home/deck/bin/redragon-lcd \
        ~/.config/systemd/user/redragon-lcd.service
  sudo rm -f /etc/udev/rules.d/99-redragon-lcd.rules
  systemctl --user daemon-reload
  sudo udevadm control --reload-rules
  log $GREEN "✓ Uninstalled"
  exit 0
fi

for c in curl sudo systemctl udevadm; do command -v $c >/dev/null || { log $RED "Missing $c"; exit 1; }; done

URL="https://github.com/RyuunosukeDS3/rust-redragon-ccw-3017/releases/latest/download/redragon-lcd-linux-amd64"
BIN="$TMP/redragon-lcd"

log $GREEN "Downloading binary..."
if ! curl -fL "$URL" -o "$BIN"; then
  log $YELLOW "Fallback build..."
  command -v cargo >/dev/null || { log $RED "cargo missing"; exit 1; }
  cd "$SCRIPT_DIR" && cargo build --release
  BIN="$SCRIPT_DIR/target/release/redragon-lcd"
fi

file "$BIN" | grep -q ELF || { log $RED "Invalid binary"; exit 1; }

log $GREEN "Installing binary..."
systemctl --user stop redragon-lcd 2>/dev/null || true
mkdir -p /home/deck/bin
install -m 755 "$BIN" /home/deck/bin/redragon-lcd

log $GREEN "Configuring service + udev..."

mkdir -p ~/.config/systemd/user/

tee ~/.config/systemd/user/redragon-lcd.service >/dev/null <<EOF
[Unit]
Description=Redragon LCD
After=network.target

[Service]
ExecStart=/home/deck/bin/redragon-lcd
Restart=always
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=default.target
EOF

echo 'SUBSYSTEM=="usb", ATTRS{idVendor}=="5131", ATTRS{idProduct}=="2007", MODE="0666"' \
| sudo tee /etc/udev/rules.d/99-redragon-lcd.rules >/dev/null

systemctl --user daemon-reload
systemctl --user enable --now redragon-lcd
sudo udevadm control --reload-rules && sudo udevadm trigger

sleep 2

if systemctl --user is-active --quiet redragon-lcd; then
  log $GREEN "✓ Running"
  systemctl --user status redragon-lcd --no-pager
else
  log $RED "Failed"
  echo "journalctl --user -u redragon-lcd -n 50"
  exit 1
fi

log $GREEN "✓ Installed"
echo "restart: systemctl --user restart redragon-lcd"
echo "logs:    journalctl --user -u redragon-lcd -f"
