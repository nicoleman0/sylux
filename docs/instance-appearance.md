# Instance appearance

Administrators save shared accent and sign-in background colors, a logo and a
sign-in image in Settings. Preview and Cancel are local. Save replaces all
appearance settings together; Reset selects built-in defaults for the next Save.
Instance naming continues to use `GET /instance` and `PATCH /instance`. Other sessions receive
updated appearance on their next page load.

`GET /instance/appearance` is public and returns only `accentColor`,
`loginBackgroundColor`, `logoUrl` and `loginImageUrl`, with `Cache-Control: no-store`.
A null value selects the built-in appearance. No credentials, file paths or
encoded image data are exposed in this response.

`PUT /instance/appearance` requires an active administrator session, same-origin
request and CSRF token. All four input fields are required:

```json
{
  "accentColor": "#336699",
  "loginBackgroundColor": null,
  "logo": { "keep": true },
  "loginImage": { "mediaType": "image/png", "data": "BASE64_BYTES" }
}
```

Colors accept six-digit hex values and are stored in uppercase. Each image input
is null to remove it, `{ "keep": true }` to retain the currently saved image, or
an upload with `mediaType` and standard base64 `data`. PNG, JPEG and WebP images
must match their claimed media type and fully decode. Animation and SVG are
rejected. Limits are 512 KiB for a logo, 2 MiB for a sign-in image, 4096 pixels per
side and 8 million pixels per image. The JSON request limit is 4 MiB. Invalid
inputs publish no changes.

Validated raster bytes are retained. `GET /instance/appearance/assets/{sha256}`
serves only currently selected images with their verified raster media type,
`nosniff` and immutable caching. Unknown or removed hashes return 404 with
`no-store`. Replacement releases the previous stored bytes; previously downloaded
public images can remain in browser caches. Missing images fall back to the
built-in appearance in the browser.

The private `metadata/appearance.json` stores colors and both encoded images in a
single file. It uses mode 0600, a synced temporary file, atomic rename and directory
sync. There are no staged asset uploads or separate image files to reconcile.
Include this file in stopped-service backups alongside `metadata/instance.json`.
A missing appearance file starts with defaults; an invalid or unsafe persisted
file fails startup validation.

Image decoding occurs before writer admission. The writer rechecks current
administrator authority immediately before publication and serializes the save
with account and session mutations. It holds no SQL transaction during filesystem
I/O. A failed response can follow a successful rename if the final directory sync
or response fails. Reload public configuration to resolve that unconfirmed save;
do not assume an error means nothing changed. These functional checks do not
qualify power-loss durability or upgrades.
