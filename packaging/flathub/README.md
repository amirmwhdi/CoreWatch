# Publishing on Flathub

`io.github.amirmwhdi.Corewatch.json` here is the manifest for the Flathub
repository. It is the same as `packaging/flatpak/` except that Corewatch is
built from a release tag on GitHub instead of the local folder.

## First submission

1. Push the release tag (for example `v0.3.0`) and put its commit hash in
   the `commit` field of the manifest:

   ```sh
   git rev-parse v0.3.0^{commit}
   ```

2. Fork <https://github.com/flathub/flathub> on GitHub, and in the fork
   create a branch from `new-pr` (not `master`).
3. Copy `io.github.amirmwhdi.Corewatch.json` and
   `packaging/flatpak/cargo-sources.json` into the root of that branch.
4. Open a pull request against the `new-pr` branch of `flathub/flathub`,
   titled `Add io.github.amirmwhdi.Corewatch`.
5. Answer the reviewers. The permissions usually get a question; the
   reasons are in `packaging/flatpak/README.md` (Network page, Processes
   page and its app icons).

The app id `io.github.amirmwhdi.*` is verified through the GitHub account
`amirmwhdi`: Flathub checks that the account owns the repository.

Before submitting, the Flathub linter can be run locally:

```sh
flatpak install -y flathub org.flatpak.Builder
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.amirmwhdi.Corewatch.json
```

## Later releases

After Flathub accepted the app it gets its own repository,
`flathub/io.github.amirmwhdi.Corewatch`. For each release, update `tag` and
`commit` there (and `cargo-sources.json` when `Cargo.lock` changed) and
merge the pull request once its test build passes.

The screenshots in the metainfo file point at the release tag, so they
appear only once the tag is pushed.
