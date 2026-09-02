# Automatic photo-library unlock

The encrypted photo library is mounted at `/mnt/Photos` from the LUKS
container `/mnt/storage/Photos/photo-library.luks`.

To configure automatic unlocking after boot, run:

```sh
scripts/enable-photo-library-auto-unlock.sh
```

The installer prompts once for the existing LUKS passphrase. It generates a
root-readable keyfile at `/etc/cryptsetup-keys.d/photo-library.key`, adds that
key to the LUKS header, and configures systemd to unlock and mount the
container after `/mnt/storage` is available.

The keyfile is stored on the already-encrypted system disk, not on the photo
volume. This provides unattended startup but means unlocking the system disk
also makes the photo library available. The installer refreshes the offline
LUKS header backup because adding a key changes the header.

The keyfile is intentionally root-only. Do not copy it or commit it.
