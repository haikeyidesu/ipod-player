# Screenshot placeholder

Add a clean screenshot of the Finder-launched app here once the macOS release has
been manually checked. Use music/artwork you own or have permission to publish;
avoid personal file paths, usernames, playlists or notifications in the image.

The development preview command renders into ignored `target/now-playing-preview/`:

```sh
cargo run --example now_playing_preview -- /path/to/your/licensed-cover.png
```

Those renders are useful for layout checks, not proof of native Finder behavior.
Do not publish the local Interstellar album artwork used during development.
