# Compiler rusty-v8 pour HermitOS (x86_64-unknown-hermit)

Ce document décrit les modifications nécessaires pour compiler la bibliothèque
statique V8 (via rusty-v8) pour la cible `x86_64-unknown-hermit`, les problèmes
rencontrés, et les choix techniques retenus.

## Vue d'ensemble

HermitOS est un unikernel POSIX-compatible mais minimaliste. V8 et son système
de build (GN/Ninja) ne le connaissent pas nativement. Trois couches doivent être
patchées :

| Couche | Dépôt | Rôle |
|--------|-------|------|
| Build system Chromium | `build/` (sous-module) | BUILDCONFIG.gn — déclaration de la cible OS |
| V8 C++ | `v8/` (sous-module) | Détection OS, platform layer, gardes POSIX |
| rusty-v8 | `build.rs` (racine) | Arguments GN, flags de link, application des patches |

## Architecture : stratégie patches

Plutôt que de forker les sous-modules `v8/` et `build/`, nous utilisons une
**stratégie de patches** (similaire à Electron pour Chromium) :

```
patches/
├── v8/
│   └── 0001-add-hermitos-platform-support.patch
└── build/
    └── 0001-add-hermitos-as-supported-target-os.patch
```

Les patches sont appliquées automatiquement par `build.rs` lors de la
compilation pour la cible `hermit`. L'application est **idempotente** : si le
patch est déjà appliqué (vérifié via `git apply --reverse --check`), il est
ignoré.

Avantages :
- Les sous-modules restent pointés vers denoland (upstream)
- Pas de forks à maintenir en sync lors des mises à jour V8
- Les modifications sont explicites et versionnées dans `patches/`

Inconvénient :
- Si V8 upstream modifie les mêmes fichiers, les patches peuvent ne plus
  s'appliquer et devront être régénérées

### Régénérer un patch

Si un patch doit être mis à jour (par exemple après un rebase V8) :

```bash
# 1. Appliquer le patch actuel
cd v8
git apply ../patches/v8/0001-add-hermitos-platform-support.patch

# 2. Faire les ajustements nécessaires
# ... éditer les fichiers ...

# 3. Régénérer le patch
git diff > ../patches/v8/0001-add-hermitos-platform-support.patch

# 4. Nettoyer
git checkout .
```

## Prérequis

- Toolchain Rust nightly avec la cible `x86_64-unknown-hermit` :
  ```bash
  rustup target add x86_64-unknown-hermit
  ```
- Clang/LLVM (le toolchain Chromium embarqué dans `third_party/llvm-build`
  convient)
- Python 3 (pour GN)
- Ninja
- Git (pour `git apply` des patches)

## Contenu des patches

### Patch `build/` : `0001-add-hermitos-as-supported-target-os.patch`

GN ne connaît pas `hermit` comme OS valide. Ce patch ajoute 3 lignes dans
`config/BUILDCONFIG.gn` :

```gn
} else if (target_os == "hermit") {
  # HermitOS : utiliser le toolchain Linux/Clang comme base
  _default_toolchain = "//build/toolchain/linux:clang_$target_cpu"
```

On réutilise le toolchain Linux car HermitOS utilise le même ABI (System V
x86_64) et le même format binaire (ELF).

### Patch `v8/` : `0001-add-hermitos-platform-support.patch`

Ce patch touche 5 fichiers :

#### `include/v8config.h` — Détection de l'OS

Déclare `V8_OS_HERMIT` et `V8_OS_POSIX`. Placé AVANT le bloc `__linux__`
car HermitOS peut aussi définir `__linux__`.

#### `src/base/platform/platform-hermit.cc` — Platform layer (nouveau fichier)

Inspiré de `platform-aix.cc`. Fonctions implémentées :
- `OS::CreateTimezoneCache()` → `PosixDefaultTimezoneCache`
- `OS::SignalCodeMovingGC()` → no-op
- `OS::AdjustSchedulingParams()` → no-op
- `OS::GetSharedLibraryAddresses()` → vecteur vide (pas de .so)
- `OS::GetFirstFreeMemoryRangeWithin()` → `nullopt`
- `OS::RemapPages()` → `false` (pas de `mremap`)
- `OS::DiscardSystemPages()` → no-op (pas de `madvise`)
- `OS::DecommitPages()` → `mmap(MAP_FIXED | MAP_ANONYMOUS | PROT_NONE)`

#### `BUILD.gn` — Enregistrement du fichier source

```gn
} else if (current_os == "hermit") {
  sources += [
    "src/base/debug/stack_trace_posix.cc",
    "src/base/platform/platform-hermit.cc",
  ]
}
```

#### `src/base/platform/platform-posix.cc` — Gardes de compilation

| API manquante | Solution |
|---------------|----------|
| `<sys/syscall.h>` | Exclure via `!V8_OS_HERMIT` |
| `DiscardSystemPages` (madvise) | `#if !V8_OS_HERMIT` |
| `DecommitPages` (mremap/madvise) | `#if !defined(_AIX) && !V8_OS_HERMIT` |
| `pthread_getattr_np` | `#elif V8_OS_HERMIT` → retourner `nullptr` |

## Modifications dans `build.rs`

### Arguments GN

```rust
if target_os == "hermit" {
    gn_args.push(r#"target_os="hermit""#.to_string());
    gn_args.push("treat_warnings_as_errors=false".to_string());
    gn_args.push("v8_enable_webassembly=false".to_string());
    gn_args.push("v8_enable_sandbox=false".to_string());
    gn_args.push("use_sysroot=false".to_string());
    gn_args.push("use_custom_libcxx=false".to_string());
    gn_args.push("enable_rust=false".to_string());
    gn_args.push("v8_enable_temporal_support=false".to_string());
}
```

Justifications :
- **`v8_enable_webassembly=false`** : Pas de signal handler pour les trap WASM
- **`v8_enable_sandbox=false`** : Nécessite 1 TB de réservation mémoire virtuelle
- **`use_sysroot=false`** : Pas de sysroot Chromium pour Hermit
- **`use_custom_libcxx=false`** : Utiliser la libc++ du toolchain
- **`enable_rust=false`** : Voir section ci-dessous
- **`v8_enable_temporal_support=false`** : Dépend de `enable_rust`
- **`treat_warnings_as_errors=false`** : Headers POSIX incomplets

### Lien C++

HermitOS n'a pas de `libc++.so` dynamique :
```rust
} else if target.contains("hermit") {
    // HermitOS: no dynamic C++ stdlib to link
}
```

## Pourquoi `enable_rust=false` dans GN

Par défaut, GN active `enable_rust=true`, ce qui compile la stdlib Rust pour
les composants internes de Chromium (`libminiz_oxide`, etc.). Deux problèmes :

### Le problème adler vs adler2

La stdlib a renommé `adler` en `adler2` (Rust 1.79+). GN choisit le nom via
`rustc_nightly_capability`, mais force `false` quand on utilise un toolchain
externe — même s'il est nightly.

### Le problème du target triple

`build/config/rust.gni` maintient une liste blanche de triples Rust.
`x86_64-unknown-hermit` n'y figure pas.

### Solution

`enable_rust=false` dans GN. La compilation Rust est gérée par Cargo ;
V8 est du C++ pur. Les composants Chromium nécessitant Rust ne sont pas
utilisés par rusty-v8.

## Compilation

```bash
# Depuis la racine de rusty-v8
V8_FROM_SOURCE=1 cargo build --target x86_64-unknown-hermit

# Avec debug GN
V8_FROM_SOURCE=1 PRINT_GN_ARGS=1 cargo build --target x86_64-unknown-hermit
```

## Limitations

- **WebAssembly** : désactivé (pas de `sigaltstack` fiable)
- **Sandbox V8** : désactivé (réservation mémoire insuffisante)
- **Profiling / stack traces** : non fonctionnel
- **Pointer compression** : devrait fonctionner (x86_64)
- **Snapshots** : devraient fonctionner (pas de dépendance OS)
- **Temporal API** : désactivé (dépend de enable_rust)
