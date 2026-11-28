# WebDAV access

WebDAV is off by default. To enable it, add `--webdav` to the server's existing `sylux serve` arguments and restart it. For Compose, use the [Docker guide](docker-beta.md#enable-webdav). For native LXC, append the flag to the existing systemd `ExecStart` command, reload the unit configuration with `sudo systemctl daemon-reload`, and restart the service.

Use the same HTTPS origin as the browser, with a certificate the client trusts. A reverse proxy must forward the DAV methods, the `Authorization` and `Destination` headers, and the upload body without rewriting the path.

Open **Account → Manage app passwords**. Choose a name for the client and select **Read only** if it should only browse and download. Create the password and copy it from the one-time display. Enter the server URL ending in `/dav/`, your Sylux account name, and that app password in the client. You can revoke each password from Account. Account password or authority changes invalidate existing app passwords. Turning WebDAV off hides app password management and keeps existing credentials stored. They work again when WebDAV is re-enabled unless they were invalidated. Revoke unwanted app passwords before turning WebDAV off.

In Thunar, enter `davs://ACCOUNT@HOST:PORT/dav/` in the location bar. In clients with separate connection settings, select **WebDAV (HTTPS)** and use `/dav/` as the starting path. Browse **Files** for the default library or open any other library you own. Create library roots in Sylux.

## Method limits

| Method | Behavior |
| --- | --- |
| `OPTIONS` | Lists the methods permitted by the app password. |
| `PROPFIND` | Looks up a resource at depth 0 or lists its immediate children at depth 1. A listing is limited to 2,048 children and 8 MiB of XML. Recursive discovery returns 403. |
| `HEAD`, `GET` | Check or download file content. File downloads support byte ranges. Collections support `HEAD` and appear in `PROPFIND`. |
| `PUT` | Creates a file with a declared `Content-Length` up to the normal 64 GiB upload limit. A name collision returns 412. Check the remote directory before retrying an interrupted upload. |
| `MKCOL` | Creates one folder inside an existing library or folder. |
| `MOVE` | Renames or moves an item within the account's libraries. The destination must be on the configured HTTPS origin. A name collision returns 412 even with `Overwrite: T`. |
| `DELETE` | Moves the file or folder to Sylux Trash. Restore it in the browser. Deleting a file or its containing folder revokes affected public file and folder links. |

Read-only passwords receive 403 for the supported write methods. An app password sees only libraries its account owns. Use the browser to open libraries shared by other accounts. WebDAV returns 403 for uploads, new folders and moves that would add content to a published library’s audience. It also returns 403 for changes that would expose content through a new public folder link audience. Use the browser for these changes.

The endpoint advertises its method subset with `DAV: sylux`. Locks, lock-token conditions, property writes, `COPY`, overwriting, resumable uploads and filesystem mounts that require locking are outside this compatibility boundary. Turn off upload timestamp preservation in clients that use property writes. Unsupported methods return 405. Clients can use the ordinary `If-Match` and `If-None-Match` ETag conditions. An interrupted PUT may have committed. If the server loses the commit result and the app password is revoked before it checks that result, the client may receive 401 even though the file exists. Check the file name and bytes with the browser or a valid app password before retrying.

Use **Refresh folder** in Sylux to see changes made while that folder was open.
