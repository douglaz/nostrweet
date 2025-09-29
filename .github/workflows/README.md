# GitHub Workflows

This directory contains automated workflows for the Nostrweet project, all powered by Nix for reproducible builds.

## 🔄 Workflows Overview

### 1. **CI (`ci.yml`)**
- **Triggers**: Push to main/develop, Pull requests
- **Purpose**: Continuous integration with comprehensive testing
- **Jobs**:
  - `check`: Code formatting, linting, and clippy checks
  - `test`: Unit tests, integration tests, and doc tests
  - `build`: Multi-target builds (GNU and musl)
  - `final-check`: Comprehensive final validation
  - `audit`: Security audit with cargo-audit

### 2. **Nix (`nix.yml`)**
- **Triggers**: Changes to flake files, manual dispatch
- **Purpose**: Nix-specific validation and Linux testing
- **Jobs**:
  - `flake-check`: Validates Nix flake configuration
  - `build-with-nix`: Tests building with Nix development shell
  - `shell-test`: Validates development environment setup
  - `cross-platform-test`: Tests on Ubuntu
  - `update-lock-file`: Checks for available dependency updates

### 3. **Release (`release.yml`)**
- **Triggers**: Tags starting with 'v', manual dispatch
- **Purpose**: Automated release creation and distribution
- **Jobs**:
  - `create-release`: Creates GitHub release with release notes
  - `build-release`: Builds release binaries with install scripts
  - `notify-discord`: Sends Discord notifications (when configured)

### 4. **Dependencies (`dependencies.yml`)**
- **Triggers**: Weekly schedule (Mondays 9 AM UTC), manual dispatch
- **Purpose**: Automated dependency maintenance
- **Jobs**:
  - `update-flake`: Updates Nix flake inputs and creates PR
  - `audit-rust-deps`: Security audit of Rust dependencies

### 5. **Documentation (`docs.yml`)**
- **Triggers**: Push to main, Pull requests
- **Purpose**: Documentation generation and validation
- **Jobs**:
  - `build-docs`: Generates Rust docs and CLI help
  - `check-links`: Validates markdown links
  - `validate-examples`: Tests code examples in documentation

### 6. **Benchmarks (`benchmarks.yml`)**
- **Triggers**: Push to main, Pull requests, manual dispatch
- **Purpose**: Performance testing and binary size tracking
- **Jobs**:
  - `benchmark`: Runs performance tests and size analysis
  - `size-comparison`: Tracks binary size over time

## 🛠️ Nix Integration

All workflows use Nix for:
- **Reproducible builds**: Identical environment across all CI runs
- **Dependency management**: Declarative dependencies in `flake.nix`
- **Cross-compilation**: Seamless musl builds for static binaries
- **Development shell**: Consistent tooling via `nix develop -c`

### Key Nix Actions Used
- `DeterminateSystems/nix-installer-action@v9`: Installs Nix
- `DeterminateSystems/magic-nix-cache-action@v2`: Speeds up builds with caching

### Common Commands
```bash
# All commands are run in the Nix development shell
nix develop -c just check      # Code quality checks
nix develop -c just build      # Build project
nix develop -c just test       # Run tests
nix develop -c just final-check # Complete validation
```

## 🔧 Configuration Files

- `.github/markdown-link-check.json`: Configuration for link checking
- `flake.nix`: Nix flake definition with development environment
- `justfile`: Build commands used by workflows
- `Cargo.toml`: Rust project configuration

## 🎯 Quality Gates

### For Pull Requests
1. ✅ Code formatting (`cargo fmt`)
2. ✅ Linting (`clippy`)
3. ✅ Type checking (`cargo check`)
4. ✅ Unit tests pass
5. ✅ Integration tests pass
6. ✅ Documentation builds
7. ✅ Nix flake validates

### For Releases
1. ✅ All CI checks pass
2. ✅ Multi-target builds successful
3. ✅ Security audit clean
4. ✅ Documentation up to date
5. ✅ Release artifacts created

## 🚀 Usage Examples

### Manual Workflow Dispatch
All workflows support manual triggering via GitHub Actions UI.

### Local Testing
You can run the same commands locally:
```bash
# Enter Nix development shell
nix develop

# Run the same checks as CI
just final-check

# Build release binaries
cargo build --release --target x86_64-unknown-linux-musl
```

### Release Process
1. Create and push a git tag: `git tag v1.0.0 && git push origin v1.0.0`
2. Release workflow automatically creates GitHub release
3. Binaries are built and uploaded
4. Release notes are generated

## 📊 Monitoring

- **Build status**: Check GitHub Actions tab
- **Security**: Weekly dependency audits
- **Performance**: Binary size tracking over time
- **Documentation**: Automatic deployment to GitHub Pages

## 🔗 Related Documentation

- [CLAUDE.md](../CLAUDE.md): Development guide with build commands
- [README.md](../README.md): User documentation
- [flake.nix](../flake.nix): Nix development environment