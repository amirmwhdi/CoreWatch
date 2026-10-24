project(
  'corewatch',
  version: '0.2.1',
  license: 'GPL-3.0-or-later',
  # Debian 13 ships Meson 1.7.
  meson_version: '>= 1.7.0',
)

i18n = import('i18n')
gnome = import('gnome')

application_id = 'io.github.amirmwhdi.Corewatch'

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
datadir = prefix / get_option('datadir')
localedir = prefix / get_option('localedir')

dependency('gtk4', version: '>= 4.18')
dependency('libadwaita-1', version: '>= 1.7')
find_program('blueprint-compiler')
cargo = find_program('cargo')

subdir('data')
subdir('po')
subdir('crates/app')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
