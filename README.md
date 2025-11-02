# frork

Inspired by [bork][bork], but I couldn't stand using bash for this, so this is
bork using Rust and Fennel instead.

[bork]: https://bork.sh/

## Usage

```
$ cat dotfiles.fnl
(local {: ok} (require :frork))

(ok :symlink "~/.gitconfig" "~/.dotfiles/.gitconfig")
```

Running through `frork`:

```
$ frork satisfy dotfiles.fnl
ok: symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
```

Compiling:

```
$ cargo build -p frork-lua --release
$ ln -s target/release/libfrork.dylib frork.so
$ fennel --compile-binary main.fnl main /opt/homebrew/lib/liblua.a /opt/homebrew/opt/lua/include/lua/
$ ./main satisfy
ok: symlink /Users/alpha/.gitconfig /Users/alpha/.dotfiles/.gitconfig
```

## TODO

- [ ] Handle `brew bundle` as a Frork declaration
- [ ] Add a parameter for automatic conflict resolution
