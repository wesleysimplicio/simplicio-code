# Local HTML preview

The Browser pane is an external tool and its support for `file://` URLs is not
owned by Simplicio Code. Do not report browser-console verification as executed
when the preview did not open.

For a generated report, expose only its temporary report directory on loopback:

```powershell
python scripts/preview_report.py C:\path\to\temporary-report
```

The helper binds to `127.0.0.1` and an ephemeral port, prints an `http://` URL,
and stops with `Ctrl+C`. It never serves the workspace root. Passing a local
`file://` URL to the validation helper fails immediately with an actionable
message instead of waiting for the Browser pane timeout:

```powershell
python scripts/preview_report.py C:\path\to\temporary-report --url file:///C:/path/to/report.html
```

If the Browser pane remains unavailable for the loopback URL, record the check
as `NOT_EXECUTED`; this workaround does not claim to fix the external pane.
