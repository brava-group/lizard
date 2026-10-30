# Online sharing and privacy

Lizard does not operate or select a default collaboration server. Online sharing is available only after you choose a server in Preferences or provide one when publishing a file. Local editing and local network sessions do not require an online server.

The standalone [`lizard-server`](server.md) is included in this repository so you can host your own service. Each operator is responsible for their server's access rules, retention, backups, contact information, and terms. Check those details before you publish a document to somebody else's server.

A server stores the files published to it and can read their contents. A share link or join code grants access according to its edit or view permission. Use `wss://` with a trusted TLS certificate when connecting over the internet. The server's settings determine whether publishing requires a server key, how long unused public files are retained, and which limits apply.

The Lizard project does not receive your files through the built-in online sharing workflow unless you explicitly configure a server operated by the project in the future. The former FigCraft endpoint is not used by default, including when older saved preferences contain its old default address.
