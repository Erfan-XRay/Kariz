# Install and update without GitHub

**English** · [فارسی](fa/offline.md)

If your server cannot reach GitHub (servers in Iran, for example, where `raw.githubusercontent.com`
does not open), the one-line install does not work. Instead you **download Kariz once on your own
computer**, copy it to the server and install from that file. The server asks GitHub for nothing.

It is the same for everything: the panel, an agent, and updates.

## 1. See the server's CPU

On the server:

```bash
uname -m
```

| Answer | The file to take |
|---|---|
| `x86_64` (most servers) | `x86_64` |
| `aarch64` | `aarch64` |
| `armv7l` | `armv7` |

## 2. Download three files

On your own computer (where GitHub opens), go to [Releases](https://github.com/Erfan-XRay/Kariz/releases/latest)
and take these **three files** from **Assets**. For version 2.8.4 on `x86_64`, for example:

```
kariz-v2.8.4-x86_64-linux.tar.gz
kariz-v2.8.4-x86_64-linux.tar.gz.sha256
kariz-v2.8.4-x86_64-linux.tar.gz.sig
```

The two small files (`.sha256` and `.sig`) are the main file's signature: the script checks with
them that nobody changed it. **Take all three**, and keep their names.

## 3. Copy them to the server

With any program that copies files to a server (WinSCP, MobaXterm, FileZilla), put the three files
in `/root` on the server. Or from your own computer's terminal:

```bash
scp kariz-v2.8.4-x86_64-linux.tar.gz* root@SERVER-IP:/root/
```

## 4. Unpack and run

On the server, as root:

```bash
cd /root
```

```bash
tar -xzf kariz-v2.8.4-x86_64-linux.tar.gz
```

```bash
bash kariz-v2.8.4-x86_64-linux/scripts/kariz.sh
```

The usual Kariz menu opens. The script finds the file next to it and says it installs from it
(*nothing is downloaded*). From here, pick what this server is for, as always.

### The panel

Pick the web panel in the menu, or directly:

```bash
bash kariz-v2.8.4-x86_64-linux/scripts/kariz.sh panel install
```

The Let's Encrypt certificate comes from Let's Encrypt, not GitHub. If that cannot be reached
either, give a certificate of your own ([web panel](panel.md)).

### An agent (connect the server to a panel)

In the panel, *Servers*, *Add server*. The command it shows needs GitHub: take **only the join
code** at its end (it starts with `kz1_`) and run on the server:

```bash
bash kariz-v2.8.4-x86_64-linux/scripts/kariz.sh --agent kz1_...
```

The agent is best on the same version as the panel.

## Updating

### The server the panel is on

Download the three files of the **new version** as above, copy them over, and run (your version
instead of 2.9.0):

```bash
cd /root
```

```bash
tar -xzf kariz-v2.9.0-x86_64-linux.tar.gz
```

```bash
bash kariz-v2.9.0-x86_64-linux/scripts/kariz.sh update
```

The panel, the core and this server's tunnels move to the new version.

### The other servers (agents)

Once the panel is updated, **you do not copy files to each server**. The panel has the new version
itself: in the panel, *Settings*, *Updates*, press **Update the servers**. The panel sends it over
the link it already has with them, and no server needs GitHub.

That is for servers with the same CPU as the panel's server (all `x86_64`, say). Update a server
with another CPU with its own file, as above:

```bash
bash kariz-v2.9.0-aarch64-linux/scripts/kariz.sh update
```

## If something goes wrong

| Message | Means |
|---|---|
| `is not a Kariz archive for this server's CPU` | The file is for another CPU, or its name changed (a browser added `(1)`, say). See step 1 and fix the name. |
| `Checksum mismatch` | The file was cut short in the download or the copy. Download it again. |
| `.sha256 and .sig are not next to the file` | The two small files are not beside the main one. Put all three in one folder. |
| `Checking the signature needs openssl` | `apt install openssl` (Ubuntu's mirrors in Iran have it too). |

If the file is somewhere else, give its path yourself:

```bash
kariz-manager install --file /path/kariz-v2.8.4-x86_64-linux.tar.gz
```

`--file` works with `install`, `update`, `panel install` and `--agent`.
