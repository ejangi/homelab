#!/usr/bin/env bash
set -Eeuo pipefail

# Configure a root-owned keyfile so systemd can unlock the photo-library LUKS
# container and mount /mnt/Photos after /mnt/storage is available at boot.
#
# This intentionally requires the current LUKS passphrase once, at
# cryptsetup luksAddKey. The passphrase is never written to disk.

container=/mnt/storage/Photos/photo-library.luks
mapper=photo-library
mountpoint=/mnt/Photos
key_dir=/etc/cryptsetup-keys.d
key_file=${key_dir}/photo-library.key
header_backup=/home/ejangi/.local/share/photos/photo-library-header.img
cryptsetup_unit="systemd-cryptsetup@$(systemd-escape "$mapper").service"
cryptsetup_dropin_dir="/etc/systemd/system/${cryptsetup_unit}.d"
legacy_cryptsetup_dropin_dir="/etc/systemd/system/systemd-cryptsetup@${mapper}.service.d"
crypttab_line="${mapper} ${container} ${key_file} luks"
fstab_line="/dev/mapper/${mapper} ${mountpoint} ext4 nodev,nosuid,noexec,nofail 0 2"

if [[ $(id -un) != ejangi ]]; then
  echo "Run this script as ejangi; it will prompt for sudo and the current LUKS passphrase." >&2
  exit 1
fi

if [[ ! -f "$container" || -L "$container" ]]; then
  echo "Expected a regular LUKS container at $container." >&2
  exit 1
fi

if [[ ! -b /dev/mapper/$mapper ]]; then
  echo "The photo library must be unlocked before adding an automatic-unlock key." >&2
  exit 1
fi

if ! mountpoint -q "$mountpoint"; then
  echo "The photo library must be mounted at $mountpoint before configuration." >&2
  exit 1
fi

sudo -v

key_exists=false
if sudo test -e "$key_file"; then
  if ! sudo test -f "$key_file"; then
    echo "Expected a regular key file at $key_file." >&2
    exit 1
  fi
  key_exists=true
  echo "Existing boot key found; completing the systemd configuration."
fi

existing_crypttab=$(sudo awk -v mapper="$mapper" '$1 == mapper { print; exit }' /etc/crypttab)
if [[ -n "$existing_crypttab" && "$existing_crypttab" != "$crypttab_line" ]]; then
  echo "Conflicting /etc/crypttab entry: $existing_crypttab" >&2
  exit 1
fi

existing_fstab=$(sudo awk -v mountpoint="$mountpoint" '$2 == mountpoint { print; exit }' /etc/fstab)
legacy_fstab_line="/dev/mapper/${mapper} ${mountpoint} ext4 nodev,nosuid,noexec,nofail,x-systemd.after=systemd-cryptsetup@${mapper}.service,x-systemd.requires=systemd-cryptsetup@${mapper}.service 0 2"
if [[ -n "$existing_fstab" && "$existing_fstab" == "$legacy_fstab_line" ]]; then
  sudo sed -i "\|^/dev/mapper/${mapper}[[:space:]]\+${mountpoint}[[:space:]]|c\\${fstab_line}" /etc/fstab
  existing_fstab="$fstab_line"
fi
if [[ -n "$existing_fstab" && "$existing_fstab" != "$fstab_line" ]]; then
  echo "Conflicting /etc/fstab entry: $existing_fstab" >&2
  exit 1
fi

if [[ "$key_exists" == false ]]; then
  sudo install -d -m 0700 "$key_dir"
  sudo dd if=/dev/urandom of="$key_file" bs=64 count=1 status=none
  sudo chmod 0600 "$key_file"

  cleanup_key() {
    sudo rm -f "$key_file"
  }
  trap cleanup_key ERR

  echo "Enter the current photo-library LUKS passphrase to add the boot key."
  sudo cryptsetup luksAddKey "$container" "$key_file"
  trap - ERR
fi

if [[ -z "$existing_crypttab" ]]; then
  printf '%s\n' "$crypttab_line" | sudo tee -a /etc/crypttab >/dev/null
fi

if [[ -z "$existing_fstab" ]]; then
  printf '%s\n' "$fstab_line" | sudo tee -a /etc/fstab >/dev/null
fi

# The encrypted container lives on /mnt/storage, a separately mounted
# filesystem. Require that mount before systemd starts cryptsetup.
if [[ "$legacy_cryptsetup_dropin_dir" != "$cryptsetup_dropin_dir" ]]; then
  sudo rm -rf "$legacy_cryptsetup_dropin_dir"
fi
sudo install -d -m 0755 "$cryptsetup_dropin_dir"
printf '%s\n' \
  '[Unit]' \
  "RequiresMountsFor=$container" \
  | sudo tee "$cryptsetup_dropin_dir/storage.conf" >/dev/null

# Adding a key changes the LUKS header, so refresh the offline header backup.
# cryptsetup refuses to overwrite an existing backup, so place the new backup
# at a path inside a root-owned temporary directory. The destination path must
# not already exist.
header_backup_dir=$(sudo mktemp -d /root/photo-library-header.XXXXXX)
header_backup_tmp="$header_backup_dir/header.img"
cleanup_header_backup() {
  sudo rm -rf "$header_backup_dir"
}
trap cleanup_header_backup ERR
sudo cryptsetup luksHeaderBackup "$container" --header-backup-file "$header_backup_tmp"
sudo install -o ejangi -g ejangi -m 0600 "$header_backup_tmp" "$header_backup"
trap - ERR
sudo rm -rf "$header_backup_dir"

sudo systemctl daemon-reload
cryptsetup_load_state=$(sudo systemctl show --value -p LoadState "$cryptsetup_unit")
mount_load_state=$(sudo systemctl show --value -p LoadState "$(systemd-escape -p --suffix=mount "$mountpoint")")
if [[ "$cryptsetup_load_state" != loaded || "$mount_load_state" != loaded ]]; then
  echo "Systemd did not generate the expected photo-library units." >&2
  exit 1
fi

echo
echo "Automatic unlock has been configured."
echo "At the next boot, systemd will unlock $container and mount $mountpoint."
echo "The Immich user service will start after its next retry once the mount exists."
