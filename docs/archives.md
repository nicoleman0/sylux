# Folder and selection downloads

Select files and folders, or open a folder's Options menu, and choose **Download** to save them as one ZIP. Single files keep their direct download. The archive keeps folder structure and includes empty folders. When a selection contains a folder and an item inside it, the folder covers both.

`POST /archives` takes `{"entry_ids": [...]}` with the session cookie, matching `Origin` and `X-CSRF-Token`. It checks the selection and returns `201` with `url`, `name` and `entry_count`. The `url` is a single-use `GET /archives/{ticket}` link for the same session, valid for 60 seconds. Each account may hold 4 unused tickets; more return `429 archive_busy`. The browser navigates to the link and streams the archive to disk.

Both requests resolve the selection in one database snapshot. The `GET` fixes the member list before streaming. Items must be active and have an active same-owner path to the private root; unknown, foreign, deleted and hidden items, and the private root, all return `404 entry_not_found`. A selection over 10,000 entries (files plus folders, including the selected items) returns `422 archive_entry_limit` with `limit`. Selected items become top-level members by name, so two with the same name from different folders return `409 archive_name_conflict`; nested paths stay unique because sibling names are unique.

Members use the stored method without compression. ZIP64 fields apply where sizes or offsets reach 4 GiB. Each payload passes the same length and SHA-256 verification as direct downloads before its bytes are sent, and CRC-32 is computed while streaming. If a payload is missing or fails verification mid-stream, the response ends without the central directory and the browser reports a failed download. Closing the download drops the stream and stops any payload verification in progress. Downloads cannot resume.

The server retains the member list and central directory in memory. Their size grows with the number and length of paths, bounded by the 10,000-entry limit; payload bytes stream in 64 KiB chunks.

## UI comparison

Inspected 15 November 2026: [Nextcloud 35's Files manual](https://docs.nextcloud.com/server/stable/user_manual/en/files/access_webgui.html) and its selection/menu screenshots support **Download** in row menus and the selection toolbar. Adopt those familiar actions. [Filebrowser's download prompt](https://github.com/filebrowser/filebrowser/blob/833d908884d5c801f30f5c098d7977177eb3a36b/frontend/src/components/prompts/Download.vue) offers archive formats; omit that choice because this scope supports ZIP only. Nextcloud visuals and Filebrowser source were inspected; neither was tested live.
