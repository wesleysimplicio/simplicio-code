# Local report preview

The Code repository does not own the Browser pane.  A generated report must
therefore never be opened as `file://` and reported as browser-verified by this
repository.

Use the bounded helper when an external browser validator is available:

```console
python scripts/preview_server.py C:\path\to\report.html --duration 30
```

The helper copies exactly that file into a temporary directory, binds an
ephemeral port on `127.0.0.1`, prints an `http://` URL, and removes the staging
directory and server when the duration ends.  It does not serve the workspace
directory.  The `file://` URI can also be passed to
`preview_server.serve_target()` by a test or another local validator.

Browser evidence remains an external concern:

- `READY` means only that the bounded HTTP endpoint started;
- `NOT_EXECUTED` means no browser was available;
- `UNVERIFIED` means the browser step still needs an external validator;
- no `PASS` browser claim is produced by this helper.

If `file://` support is later fixed in the Browser pane, callers may remove the
rewrite at their integration boundary.  Until then, the rollback path is to
stop the helper and discard its temporary directory; no workspace file is
modified.
