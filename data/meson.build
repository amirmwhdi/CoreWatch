# Desktop entry, AppStream metadata and icon. The UI itself (Blueprint and
# style.css) is compiled into the binary by crates/app/build.rs.

# Both are installed with the translations from po/ merged in.
desktop_file = i18n.merge_file(
  input: application_id + '.desktop',
  output: application_id + '.desktop',
  type: 'desktop',
  po_dir: meson.project_source_root() / 'po',
  install: true,
  install_dir: datadir / 'applications',
)

metainfo_file = i18n.merge_file(
  input: application_id + '.metainfo.xml',
  output: application_id + '.metainfo.xml',
  po_dir: meson.project_source_root() / 'po',
  install: true,
  install_dir: datadir / 'metainfo',
)

install_data(
  'icons' / application_id + '.svg',
  install_dir: datadir / 'icons/hicolor/scalable/apps',
)

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test('validate-desktop', desktop_file_validate, args: [desktop_file])
endif

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test(
    'validate-metainfo',
    appstreamcli,
    args: ['validate', '--no-net', '--explain', metainfo_file],
  )
endif
