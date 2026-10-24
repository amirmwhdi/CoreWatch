# Cargo builds the binary; Meson only tells it where things get installed and
# copies the result into place.

if get_option('profile') == 'development'
  rust_profile = 'debug'
  cargo_profile_args = []
else
  rust_profile = 'release'
  cargo_profile_args = ['--release']
endif

cargo_target_dir = meson.project_build_root() / 'target'

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'corewatch',
  console: true,
  install: true,
  install_dir: bindir,
  env: {
    'CARGO_TARGET_DIR': cargo_target_dir,
    'COREWATCH_LOCALEDIR': localedir,
  },
  command: [
    find_program('sh'),
    '-c',
    'c="$1"; m="$2"; src="$3"; dst="$4"; shift 4; "$c" build --manifest-path "$m" -p corewatch "$@" && cp "$src" "$dst"',
    'sh',
    cargo,
    meson.project_source_root() / 'Cargo.toml',
    cargo_target_dir / rust_profile / 'corewatch',
    '@OUTPUT@',
    cargo_profile_args,
  ],
)
