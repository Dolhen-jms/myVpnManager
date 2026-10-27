# myVpnManager

Client [openconnect](https://www.infradead.org/openconnect/) en terminal, à la `nmtui` :
une liste de connexions, des boutons, l'état et le journal d'openconnect. Avec un module
pour la barre [wayle](https://github.com/wayle-rs/wayle) qui suit le VPN en direct.

- **Connexions dans un fichier**, `~/.config/myvpnmanager/config.toml`, modifiables aussi depuis
  l'interface (protocoles d'openconnect : AnyConnect, GlobalProtect, Pulse, Fortinet…).
- **Mots de passe chiffrés** : un mot de passe maître, demandé au lancement, donne la clé
  (Argon2id) ; les mots de passe des connexions sont rangés chiffrés dans `config.toml`
  (XChaCha20-Poly1305) et déchiffrés seulement au moment de lancer openconnect.
- **Le VPN survit à l'interface** : openconnect tourne dans sa propre session ; on peut
  fermer la fenêtre (ou le terminal), il reste connecté. En rouvrant myVpnManager, on le retrouve.
- **Thème du bureau** suivi à chaud (même palette que la barre). Interface en français.

## Installation

Linux seulement. Il faut `sudo` (openconnect a besoin des droits root) ; openconnect est
fourni par le paquet Nix s'il n'est pas déjà installé.

### Paquet Nix

`default.nix` construit le paquet : le binaire, enveloppé pour trouver openconnect, et une
entrée de menu (`myvpnmanager.desktop`, lancée dans un terminal).

Depuis le dossier :

```sh
nix-build          # → ./result/bin/myvpnmanager
nix-env -f . -i    # ou : l'installer dans son profil
```

Ou dans `configuration.nix` (NixOS) :

```nix
environment.systemPackages = [
  (pkgs.callPackage /home/dolhen/.myapps/myVpnManager { })
];
```

puis `sudo nixos-rebuild switch`. Dans une configuration en flake, le chemin doit être dans
le flake ou passé en `--impure`, ou le dépôt ajouté comme `input`.

Mettre à jour : refaire `nix-env -f . -i` (ou `nixos-rebuild switch`). Désinstaller :
`nix-env -e myvpnmanager`.

Les tests (`crypto`) tournent pendant la construction ; celui de l'interface est sauté (il lit
la configuration de l'utilisateur).

### Depuis les sources

Aucune dépendance système : `cargo` suffit (`nix-shell` le fournit sous NixOS).

```sh
cargo build --release
install -m755 target/release/myvpnmanager ~/.local/bin/myvpnmanager
```

## Utilisation

```sh
myvpnmanager                     # interface ; le VPN reste connecté quand on la quitte
myvpnmanager status              # état des connexions
myvpnmanager status --json       # état pour une barre
myvpnmanager status --watch      # pareil, une ligne JSON à chaque changement
myvpnmanager open                # ouvre l'interface dans un terminal (ou la met au premier plan)
myvpnmanager disconnect          # coupe le VPN actif (sudo demande son mot de passe)
```

| Touches | Action |
|---|---|
| `⏎` `espace` `c`, double clic | connecter / déconnecter (annuler pendant la connexion) |
| `d` | déconnecter le VPN actif |
| `⇥` `←` `→` | passer de la liste aux boutons |
| `↑` `↓` `j` `k` | se déplacer |
| `a` `e` `x` | ajouter, modifier, supprimer une connexion |
| `m` | changer le mot de passe maître |
| `r` | relire `config.toml` |
| `?` | aide |
| `q` `Échap` | quitter (le VPN reste connecté) |

Un seul VPN à la fois : se connecter à un autre coupe d'abord celui qui est actif.
Le mot de passe sudo, s'il en faut un, est demandé dans l'interface et gardé en mémoire
tant qu'elle est ouverte.

## Configuration

`~/.config/myvpnmanager/config.toml` (mode 600), une table `[[vpn]]` par connexion :

```toml
terminal = "kitty --class myvpnmanager --title myvpnmanager -e"   # pour `myvpnmanager open`

[[vpn]]
name = "ul"                          # nom affiché ; interface réseau vpn-ul
host = "vpn.exemple.fr"
protocol = "anyconnect"              # anyconnect, gp, nc, pulse, f5, fortinet, array
authgroup = "Universite"             # --authgroup (facultatif)
user = "moi@etu"                     # --user (facultatif)
# password-enc = "v1:…"              # écrit par myVpnManager ; absent : demandé à chaque connexion
# password = "…"                     # en clair : chiffré puis effacé au lancement suivant
# password-command = "pass show vpn" # ou une commande qui l'affiche
# extra-args = ["--servercert=pin-sha256:…"]
```

La section `[vault]` (sel, paramètres d'Argon2id, valeur de contrôle) est écrite par myVpnManager
au premier lancement. **Mot de passe maître oublié** : supprimer `[vault]` et les lignes
`password-enc` ; un nouveau mot de passe maître est demandé, les mots de passe VPN sont à
ressaisir.

**Thème** (facultatif) : si `~/.local/state/theme/current/wayle.scss` existe, sa palette
(`--palette-*`) colore l'interface, et un changement de thème est suivi à chaud. Sinon,
couleurs du terminal.

Les journaux d'openconnect sont dans `$XDG_RUNTIME_DIR/myvpnmanager/vpn-<nom>.log` (effacés au
redémarrage).

## Barre wayle

Dans `~/.config/wayle/config.toml`, ajouter `"custom-vpn"` à une disposition (`right = […]`)
et :

```toml
[[modules.custom]]
id = "vpn"
command = "myvpnmanager status --watch"
mode = "watch"
icon-map = { off = "ld-shield-symbolic", connecting = "ld-shield-ellipsis-symbolic", on = "ld-shield-check-symbolic" }
icon-color = "fg-muted"
label-color = "fg-muted"
button-bg-color = "transparent"
label-max-length = 16
left-click = "myvpnmanager open"
```

Les icônes : `wayle icons install lucide shield shield-check shield-ellipsis`. Le bouton
montre le bouclier seul hors connexion, le nom du VPN une fois connecté (IP et durée dans
l'infobulle) ; un clic ouvre l'interface.

Fenêtre flottante sous Hyprland (facultatif) :

```
windowrule = float on, size 900 600, center on, match:class ^(myvpnmanager)$
```

## Fonctionnement

openconnect est lancé par `sudo -S` dans une nouvelle session (`setsid`), avec
`--background`, `--non-inter` et `--interface=vpn-<nom>` ; sa sortie va dans le journal. Le
mot de passe sudo puis celui du VPN passent par son entrée standard, jamais par la ligne de
commande. L'état n'est rangé nulle part : myVpnManager retrouve les processus openconnect dans
`/proc` (y compris ceux lancés à la main) et l'adresse de leur interface. La déconnexion
envoie `SIGINT` (déconnexion propre), puis `SIGTERM` s'il traîne.
