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
| rusty-v8 | `build.rs` (racine) | Arguments GN, flags de link |

## Prérequis

- Toolchain Rust nightly avec la cible `x86_64-unknown-hermit` :
  ```bash
  rustup target add x86_64-unknown-hermit
  ```
- Clang/LLVM (le toolchain Chromium embarqué dans `third_party/llvm-build`
  convient)
- Python 3 (pour GN)
- Ninja

## Modifications détaillées

### 1. `build/config/BUILDCONFIG.gn` — Déclarer la cible

GN ne connaît pas `hermit` comme OS valide. Sans ce patch, `gn gen` échoue
immédiatement.

```gn
# Après le bloc "zos", ajouter :
} else if (target_os == "hermit") {
  # HermitOS : utiliser le toolchain Linux/Clang comme base
  _default_toolchain = "//build/toolchain/linux:clang_$target_cpu"
```

On réutilise le toolchain Linux car HermitOS utilise le même ABI (System V
x86_64) et le même format binaire (ELF). Clang sait déjà produire du code
pour cette cible.

### 2. `v8/include/v8config.h` — Détection de l'OS

V8 utilise des macros `V8_OS_*` pour toute la logique conditionnelle. Il faut
déclarer `V8_OS_HERMIT` et `V8_OS_POSIX`.

```c
// IMPORTANT : placer AVANT le bloc __linux__ car HermitOS définit aussi
// __linux__ dans certains cas.

#elif defined(__hermit__)
# define V8_OS_HERMIT 1
# define V8_OS_POSIX 1
# define V8_OS_STRING "hermit"
```

Pour les headers d'inclusion au début du fichier :
```c
#elif defined(__hermit__)
// HermitOS: no special platform headers needed
```

### 3. `v8/src/base/platform/platform-hermit.cc` — Platform layer

V8 a besoin d'une implémentation de l'interface OS pour chaque plateforme.
Le fichier est inspiré de `platform-aix.cc` (autre plateforme POSIX limitée) :

Fonctions implémentées :
- `OS::CreateTimezoneCache()` → `PosixDefaultTimezoneCache`
- `OS::SignalCodeMovingGC()` → no-op
- `OS::AdjustSchedulingParams()` → no-op
- `OS::GetSharedLibraryAddresses()` → vecteur vide (pas de .so)
- `OS::GetFirstFreeMemoryRangeWithin()` → `nullopt` (pas de `/proc/self/maps`)
- `OS::RemapPages()` → `false` (pas de `mremap`)
- `OS::DiscardSystemPages()` → no-op (pas de `madvise`)
- `OS::DecommitPages()` → `mmap(MAP_FIXED | MAP_ANONYMOUS | PROT_NONE)` pour
  libérer les pages physiques

### 4. `v8/BUILD.gn` — Enregistrer le fichier source

```gn
  } else if (current_os == "hermit") {
    sources += [
      "src/base/debug/stack_trace_posix.cc",
      "src/base/platform/platform-hermit.cc",
    ]
  }
```

### 5. `v8/src/base/platform/platform-posix.cc` — Gardes de compilation

Plusieurs parties du code POSIX partagé utilisent des APIs absentes de HermitOS :

| API manquante | Fichier | Solution |
|---------------|---------|----------|
| `<sys/syscall.h>` | platform-posix.cc:77 | Exclure via `!V8_OS_HERMIT` |
| `DiscardSystemPages` (madvise) | platform-posix.cc:618 | `#if !V8_OS_HERMIT` (fourni dans platform-hermit.cc) |
| `DecommitPages` (mremap/madvise) | platform-posix.cc:652 | `#if !defined(_AIX) && !V8_OS_HERMIT` |
| `pthread_getattr_np` | platform-posix.cc:1441 | `#elif V8_OS_HERMIT` → retourner `nullptr` |

### 6. `build.rs` — Arguments GN côté Rust

```rust
if target_os == "hermit" {
    gn_args.push(r#"target_os="hermit""#.to_string());
    gn_args.push("treat_warnings_as_errors=false".to_string());
    gn_args.push("v8_enable_webassembly=false".to_string());
    gn_args.push("v8_enable_sandbox=false".to_string());
    gn_args.push("use_sysroot=false".to_string());
    gn_args.push("use_custom_libcxx=false".to_string());
}
```

Justifications :
- **`v8_enable_webassembly=false`** : HermitOS n'a pas de signal handler pour
  les trap WASM (pas de `sigaltstack` fiable)
- **`v8_enable_sandbox=false`** : Le sandbox V8 nécessite des fonctionnalités
  mémoire avancées (reservation de 1 TB d'espace d'adressage virtuel)
- **`use_sysroot=false`** : Pas de sysroot Chromium pour Hermit
- **`use_custom_libcxx=false`** : Utiliser la libc++ du toolchain, pas celle
  de Chromium
- **`treat_warnings_as_errors=false`** : Certains headers POSIX incomplets
  génèrent des warnings

Pour le link, HermitOS n'a pas de `libc++.so` dynamique :
```rust
} else if target.contains("hermit") {
    // HermitOS: no dynamic C++ stdlib to link
}
```

## Problème non traité : `enable_rust` dans GN

Par défaut, le build GN de V8 active `enable_rust=true`, ce qui tente de
compiler la stdlib Rust via GN (pour les composants internes de Chromium comme
`libminiz_oxide`). Cela pose plusieurs problèmes pour HermitOS :

### Le problème adler vs adler2

La stdlib Rust a renommé la crate `adler` en `adler2` (Rust 1.79+). Le système
de build GN de Chromium choisit le nom en fonction de `rustc_nightly_capability` :
- `true` → cherche `libadler2`
- `false` → cherche `libadler`

Quand on utilise un toolchain Rust externe (`use_chromium_rust_toolchain=false`),
GN force `rustc_nightly_capability=false` même si le toolchain est un nightly
récent.

### Le problème du target triple

Le fichier `build/config/rust.gni` maintient une liste blanche de triples Rust
connus. `x86_64-unknown-hermit` n'y figure pas, ce qui cause des erreurs dans
la résolution du sysroot Rust.

### Solution retenue

**Désactiver `enable_rust` dans GN** (`enable_rust=false`). La compilation Rust
est déjà gérée par Cargo côté rusty-v8 ; V8 lui-même est du C++ pur. Activer
Rust dans GN ne sert qu'à des composants internes de Chromium (compression,
etc.) qui ne sont pas nécessaires pour rusty-v8.

Cela rend les problèmes adler/adler2 et target triple non pertinents.

> **Note** : Cet argument n'est pas encore ajouté dans le commit actuel. Il
> faudra ajouter `gn_args.push("enable_rust=false".to_string());` dans le bloc
> hermit de `build.rs` si la compilation GN échoue sur les composants Rust.

## Compilation

```bash
# Depuis la racine de rusty-v8
cargo build --target x86_64-unknown-hermit

# Ou avec des variables d'environnement pour le debug
V8_FROM_SOURCE=1 PRINT_GN_ARGS=1 cargo build --target x86_64-unknown-hermit
```

## État actuel et limitations

- **WebAssembly** : désactivé. Pourrait être réactivé si HermitOS implémente
  `sigaltstack` et les signal handlers complets.
- **Sandbox V8** : désactivé. Nécessiterait des modifications significatives
  du virtual memory allocator de V8.
- **Profiling / stack traces** : `GetSharedLibraryAddresses` retourne un
  vecteur vide ; `ObtainCurrentThreadStackStart` retourne `nullptr`. Les
  profilers V8 ne fonctionneront pas.
- **Pointer compression** : devrait fonctionner (activé par défaut sur x86_64).
- **Snapshots** : devraient fonctionner (pas de dépendance OS).

## Références

- [RustyHermit](https://github.com/hermitcore/rusty-hermit) — runtime Rust
  pour HermitOS
- [V8 Platform API](https://v8.dev/docs/embed#platform) — interface plateforme
- Commits similaires : `platform-aix.cc`, `platform-zos.cc` dans V8
