## Install

All releases are available [here](https://github.com/dstrml/ade/releases/latest).

### macOS (Apple Silicon and Intel)

1. Download the installer for your Mac, open it, and drag Ade into your Applications folder.
    - [**Apple Silicon**](https://github.com/dstrml/ade/releases/latest/download/Ade-aarch64.dmg) (M1 and later)
    - [**Intel**](https://github.com/dstrml/ade/releases/latest/download/Ade-x86_64.dmg)

   Ade is not signed with an Apple Developer certificate yet, so macOS will block it the first time you open it.
2. Open Terminal and run:

   ```sh
   xattr -d com.apple.quarantine /Applications/Ade.app
   ```

> If you'd rather not use Terminal: open Ade once (you'll see a "Ade can't be opened" or "Apple could not verify" message), then go to **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to Ade. Confirm with your password.

3. Open Ade normally.

#### Command line

To open files and folders from Terminal with `ade`, choose **Ade → Install CLI** from the menu bar. It adds `ade` to `/usr/local/bin` and may ask for your password.

If that fails, for example because your account isn't an admin, add it yourself without a password:

```sh
mkdir -p ~/.local/bin
ln -sf /Applications/Ade.app/Contents/MacOS/cli ~/.local/bin/ade
```

If `ade` still isn't found, add `~/.local/bin` to your `PATH` by adding this line to `~/.zshrc`, then open a new Terminal window:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

### Linux (x86-64 and AArch64)

Run this in a terminal:

```sh
curl -fsSL https://ade.dstrml.org/install.sh | sh
```

It installs Ade into `~/.local`, adds the `ade` command, and adds Ade to your app menu with its icon. Run it again any time to reinstall. Ade keeps itself up to date after that.

#### Manual install

1. Download the tarball for your platform.
    - [**Linux x86-64**](https://github.com/dstrml/ade/releases/latest/download/ade-linux-x86_64.tar.gz)
    - [**Linux AArch64**](https://github.com/dstrml/ade/releases/latest/download/ade-linux-aarch64.tar.gz)

2. Unpack its contents into `~/.local`:

```sh
tar -xzf ade-linux-$(uname -m).tar.gz -C ~/.local
ln -sf ~/.local/ade.app/bin/ade ~/.local/bin/ade
```

Make sure `~/.local/bin` is on your `PATH`, then run `ade`. This doesn't add Ade to your app menu; the install command above does.

#### WSL2

On WSL2, Ade draws with the CPU by default because the WSL GPU driver isn't fully standards-compliant. To try the GPU instead, start Ade with:

```sh
WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 ade
```

### Windows (x86-64)

Download the [installer](https://github.com/dstrml/ade/releases/latest/download/Ade-x86_64.exe) and click to run. The installer isn't code-signed, so Windows SmartScreen may warn you. Click **More info**, then **Run anyway**.
