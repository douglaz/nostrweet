# 🔄 GitHub Workflows with Nix

This document provides a comprehensive overview of the GitHub workflows implemented for the Nostrweet project, all powered by Nix for reproducible builds.

## 🎯 Quick Start

### Testing Workflows Locally
```bash
# Enter Nix development shell
nix develop

# Run core checks (same as CI)
just build
just test
just check
just clippy

# Test musl build (for releases)
cargo build --release --target x86_64-unknown-linux-musl

# Validate Nix setup
nix flake check
```

### Workflow Files Created

| File | Purpose | Triggers |
|------|---------|----------|
| `ci.yml` | Continuous Integration | Push, PR |
| `nix.yml` | Nix-specific validation | Flake changes |
| `release.yml` | Automated releases | Git tags |
| `dependencies.yml` | Dependency updates | Weekly, manual |
| `docs.yml` | Documentation | Push to main, PR |
| `benchmarks.yml` | Performance testing | Push, PR, manual |

## 🛠️ Key Features

### ❄️ Nix Integration
- **Reproducible builds** across all CI environments
- **Declarative dependencies** in `flake.nix`
- **Cross-compilation** for musl targets
- **Consistent tooling** via `nix develop -c`

### 🚀 Automation
- **Automatic releases** on git tags
- **Weekly dependency updates** with PR creation
- **Security audits** with vulnerability reporting
- **Documentation deployment** to GitHub Pages
- **Binary size tracking** over time

### 🧪 Quality Assurance
- **Multi-stage testing**: unit, integration, doc tests
- **Code quality**: formatting, linting, clippy
- **Linux-focused validation**: Ubuntu with both GNU and musl targets
- **Performance benchmarks** and size analysis

## 📊 Workflow Details

### CI Workflow (`ci.yml`)
**Purpose**: Main continuous integration pipeline

**Jobs**:
- `check`: Code formatting, linting, clippy
- `test`: Unit, integration, and doc tests  
- `build`: Multi-target builds (GNU, musl)
- `final-check`: Comprehensive validation
- `audit`: Security audit with cargo-audit

**Key Commands**:
```bash
nix develop -c cargo fmt --all -- --check
nix develop -c just check
nix develop -c just clippy
nix develop -c just test
nix develop -c cargo build --release --target x86_64-unknown-linux-musl
```

### Nix Workflow (`nix.yml`)
**Purpose**: Validate Nix flake and Linux compatibility

**Jobs**:
- `flake-check`: Validates flake configuration
- `build-with-nix`: Tests building in Nix shell
- `shell-test`: Validates development environment
- `cross-platform-test`: Tests on Ubuntu
- `update-lock-file`: Checks for dependency updates

### Release Workflow (`release.yml`)
**Purpose**: Automated release creation and distribution

**Jobs**:
- `create-release`: Creates GitHub release with notes
- `build-release`: Builds release binaries with install scripts
- `notify-discord`: Sends notifications (when configured)

**Artifacts Created**:
- `nostrweet-v1.0.0-x86_64-unknown-linux-gnu.tar.gz`
- `nostrweet-v1.0.0-x86_64-unknown-linux-musl.tar.gz`
- SHA256 checksums
- Install scripts

### Dependencies Workflow (`dependencies.yml`)
**Purpose**: Automated dependency maintenance

**Jobs**:
- `update-flake`: Updates Nix inputs, creates PR
- `audit-rust-deps`: Security audit of Rust dependencies

**Schedule**: Every Monday at 09:00 UTC

### Documentation Workflow (`docs.yml`)
**Purpose**: Documentation generation and validation

**Jobs**:
- `build-docs`: Generates Rust docs and CLI help
- `check-links`: Validates markdown links
- `validate-examples`: Tests code examples

**Deployment**: Automatic to GitHub Pages on main branch

### Benchmarks Workflow (`benchmarks.yml`)
**Purpose**: Performance testing and size tracking

**Jobs**:
- `benchmark`: Performance tests and analysis
- `size-comparison`: Binary size tracking over time

## 🔧 Configuration

### Nix Flake (`flake.nix`)
```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  
  outputs = { nixpkgs, rust-overlay, ... }: {
    devShells.default = pkgs.mkShell {
      packages = with pkgs; [
        rust-bin.stable.latest.default
        just
        pkg-config
        openssl
        # ... more tools
      ];
    };
  };
}
```

### Just Commands (`justfile`)
```just
# Core commands used by workflows
build:     cargo build --workspace --all-targets
check:     cargo check --workspace --all-targets  
test:      cargo test
clippy:    cargo clippy --workspace --all-targets --all-features -- --deny warnings
format:    cargo fmt --all
final-check: lint clippy test
```

## 🎯 Quality Gates

### Pull Request Requirements
- ✅ Code formatting (`cargo fmt`)
- ✅ Linting (`just clippy`)
- ✅ Type checking (`just check`)
- ✅ All tests pass (`just test`)
- ✅ Nix flake validates
- ✅ Documentation builds
- ✅ Multi-target compilation

### Release Requirements
- ✅ All CI checks pass
- ✅ Security audit clean
- ✅ Linux builds successful (both GNU and musl)
- ✅ Documentation up to date

## 🔍 Monitoring & Observability

### Build Status
- All workflows provide detailed status in GitHub Actions
- PR comments with benchmark results
- Binary size tracking over time

### Security
- Weekly dependency audits
- Vulnerability reporting in artifacts
- Security-focused cargo audit integration

### Performance
- Binary size monitoring
- Build time tracking
- Linux target compilation validation

## 🚀 Usage Examples

### Manual Release
```bash
# Create and push a tag
git tag v1.0.0
git push origin v1.0.0

# Release workflow automatically:
# 1. Creates GitHub release
# 2. Builds binaries for multiple targets
# 3. Uploads release artifacts
# 4. Generates release notes
```

### Testing Changes Locally
```bash
# Use same environment as CI
nix develop

# Run all checks
just final-check

# Test specific functionality
cargo test specific_test
```

### Updating Dependencies
```bash
# Update Nix inputs
nix flake update

# Update Rust dependencies  
cargo update

# Both are automated weekly via dependencies.yml
```

## 🔗 Integration Points

### GitHub Features Used
- **Actions**: All workflows
- **Releases**: Automated release creation
- **Pages**: Documentation deployment
- **Issues**: Dependency update notifications
- **PR Comments**: Benchmark results

### External Services
- **Nix Cache**: Speeds up builds significantly
- **GitHub Container Registry**: For caching (future)
- **Discord**: Release notifications (optional)

## 📈 Benefits

### For Developers
- **Consistent environment** across local and CI
- **Fast feedback** on code quality
- **Automated maintenance** of dependencies
- **Comprehensive testing** before merge

### For Users
- **Reliable releases** with automated testing
- **Multiple binary formats** (GNU libc, musl)
- **Up-to-date documentation** 
- **Security-audited** dependencies

### For Project Maintenance
- **Zero-config releases** on git tags
- **Automated dependency updates**
- **Performance regression detection**
- **Documentation always current**

This workflow setup provides a robust foundation for maintaining high code quality while minimizing manual maintenance overhead, all powered by Nix's reproducible build system.