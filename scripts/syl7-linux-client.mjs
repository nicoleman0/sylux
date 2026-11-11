import fs from "node:fs";
import https from "node:https";
import crypto from "node:crypto";
const ca = fs.readFileSync(process.argv[3]);
const origin = process.argv[4];
let cookie = "",
  csrf = "";
function req(method, path, body, extra = {}) {
  return new Promise((resolve, reject) => {
    let b =
      body == null
        ? Buffer.alloc(0)
        : Buffer.isBuffer(body)
          ? body
          : Buffer.from(JSON.stringify(body));
    let headers = {
      Origin: origin,
      ...(cookie ? { Cookie: cookie } : {}),
      ...(csrf ? { "X-CSRF-Token": csrf } : {}),
      ...(body != null
        ? {
            "Content-Type": Buffer.isBuffer(body)
              ? "application/octet-stream"
              : "application/json",
            "Content-Length": b.length,
          }
        : {}),
      ...extra,
    };
    let q = https.request(origin + path, { method, headers, ca }, (r) => {
      let chunks = [];
      r.on("data", (x) => chunks.push(x));
      r.on("end", () => {
        let data = Buffer.concat(chunks);
        if (r.headers["set-cookie"])
          cookie = r.headers["set-cookie"]
            .map((x) => x.split(";")[0])
            .join("; ");
        resolve({
          status: r.statusCode,
          headers: r.headers,
          data,
          json: () => JSON.parse(data),
        });
      });
    });
    q.on("error", reject);
    q.end(b);
  });
}
function ok(r, status) {
  if (r.status !== status)
    throw Error(`HTTP ${r.status} expected ${status}: ${r.data}`);
  return r;
}
async function login(handle, password) {
  cookie = "";
  csrf = "";
  let r = ok(await req("GET", "/auth/csrf"), 200);
  csrf = r.json().csrf_token;
  r = ok(await req("POST", "/auth/local", { handle, password }), 200);
  csrf = r.json().csrf_token;
}
const users = [
  {
    handle: "owner",
    password: "correct horse battery staple",
    name: "owner-file.txt",
    content: "owner private bytes\n",
  },
  {
    handle: "other",
    password: "other account password",
    name: "other-file.txt",
    content: "other private bytes\n",
  },
];
async function upload(user) {
  let root = ok(await req("GET", "/folders/children"), 200).json().root;
  let bytes = Buffer.from(user.content);
  let sha256 = crypto.createHash("sha256").update(bytes).digest("hex");
  let input = {
    scope: "upload_create",
    parent_id: root.id,
    parent_etag: root.etag,
    name: user.name,
    length: bytes.length,
    sha256,
  };
  let ticket = ok(await req("POST", "/mutation-tickets", input), 201).headers[
    "x-mutation-id"
  ];
  let { scope, ...admission } = input;
  let id = ok(
    await req("POST", "/uploads", admission, {
      "X-Mutation-ID": ticket,
      "If-None-Match": "*",
    }),
    201,
  ).json().upload_id;
  ok(
    await req("PUT", `/uploads/${id}/content`, bytes, {
      "X-Mutation-ID": ticket,
    }),
    200,
  );
  let file = ok(
    await req(
      "POST",
      `/uploads/${id}/commit`,
      { sha256, parent_etag: root.etag },
      { "X-Mutation-ID": ticket, "If-None-Match": "*" },
    ),
    201,
  ).json();
  return { root: root.id, file: file.id };
}
async function verify(state) {
  for (let i = 0; i < 2; i++) {
    let u = users[i],
      own = state[i],
      other = state[1 - i];
    await login(u.handle, u.password);
    let list = ok(await req("GET", "/folders/children"), 200).json();
    if (
      list.root.id !== own.root ||
      list.items.length !== 1 ||
      list.items[0].id !== own.file ||
      list.items[0].name !== u.name
    )
      throw Error("inventory mismatch");
    if (
      !ok(await req("GET", `/entries/${own.file}/content`), 200).data.equals(
        Buffer.from(u.content),
      )
    )
      throw Error("bytes differ");
    ok(await req("GET", `/folders/${other.root}/children`), 404);
    ok(await req("GET", `/entries/${other.file}/content`), 404);
  }
  console.error(
    "both accounts: inventory, bytes and cross-account denials passed",
  );
}
let mode = process.argv[2];
if (mode === "prepare") {
  let token = fs.readFileSync(
    process.argv[5] + "/secrets/setup.authorization",
    "utf8",
  );
  ok(
    await req(
      "POST",
      "/setup/admin",
      { handle: users[0].handle, password: users[0].password },
      { "X-Setup-Authorization": token },
    ),
    201,
  );
  await login(users[0].handle, users[0].password);
  ok(
    await req("POST", "/accounts", {
      handle: users[1].handle,
      password: users[1].password,
    }),
    204,
  );
  let state = [];
  for (let u of users) {
    await login(u.handle, u.password);
    state.push(await upload(u));
  }
  await verify(state);
  fs.writeFileSync(process.argv[6], JSON.stringify(state), { mode: 0o600 });
} else if (mode === "verify") {
  await verify(JSON.parse(fs.readFileSync(process.argv[6], "utf8")));
} else throw Error("bad mode");
