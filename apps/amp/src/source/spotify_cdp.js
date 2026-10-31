// Makepad Amp's bridge into the Spotify desktop client (Linux), evaluated in
// the client's own page (https://xpui.app.spotify.com/index.html) over the
// Chrome DevTools Protocol. It modifies nothing: it finds the client's
// Platform object in the React tree and calls the client's own internal APIs
// (PlayerAPI, RootlistAPI, PlaylistAPI, LibraryAPI, ConnectAPI, GraphQL).
//
// Every answer has the JSON shape the macOS/Windows `spotify_cli` prints, so
// the Rust side reads both with the same parsers (source/spotify.rs).
//
// Evaluated as `(<this function>)(tag)`: `tag` is a hash of this text, so a
// changed bridge replaces the one a page already holds. Calls are
// `window.__makepadAmp.call(name, argsJson)` and answer "ok:<json>" or
// "err:<words>". `probe` says what a Spotify update took away.
(function (tag) {
  "use strict";
  const BRIDGE = 1;

  const findPlatform = () => {
    const root = document.getElementById("main");
    if (!root) return null;
    const key = Object.keys(root).find((k) => k.startsWith("__reactContainer"));
    if (!key) return null;
    const seen = new Set();
    const stack = [root[key]];
    while (stack.length) {
      const fiber = stack.pop();
      if (!fiber || seen.has(fiber)) continue;
      seen.add(fiber);
      const value = fiber.memoizedProps && fiber.memoizedProps.value;
      if (value && typeof value === "object" && typeof value.getRegistry === "function" && typeof value.getSession === "function") return value;
      if (fiber.child) stack.push(fiber.child);
      if (fiber.sibling) stack.push(fiber.sibling);
    }
    return null;
  };

  let platform = null;
  const apis = new Map();
  const api = (name) => {
    if (apis.has(name)) return apis.get(name);
    if (!platform) platform = findPlatform();
    if (!platform) return null;
    const registry = platform.getRegistry();
    let found = null;
    for (const symbol of registry._map.keys()) {
      if (symbol.description === name) {
        found = registry.resolve(symbol);
        break;
      }
    }
    if (found) apis.set(name, found);
    return found;
  };
  const need = (name) => {
    const found = api(name);
    if (!found) throw new Error(`the client has no ${name} (a Spotify update?)`);
    return found;
  };

  // GraphQL operations by name, read from the loaded modules' sources
  // (`new X("name","query","<sha256>",null)`); nothing is executed.
  let graphql = null;
  const gql = (name) => {
    if (!graphql) {
      graphql = {};
      let require = null;
      const chunks = window.rspackChunkclient_web || window.webpackChunkclient_web;
      if (chunks) chunks.push([[Symbol("makepad-amp")], {}, (r) => { require = r; }]);
      if (require && require.m) {
        const pattern = /\("(\w+)","(query|mutation)","([0-9a-f]{64})"/g;
        for (const id of Object.keys(require.m)) {
          const source = String(require.m[id]);
          let match;
          while ((match = pattern.exec(source))) graphql[match[1]] = { name: match[1], operation: match[2], sha256Hash: match[3], value: null };
        }
      }
    }
    const definition = graphql[name];
    if (!definition) throw new Error(`the client has no ${name} query (a Spotify update?)`);
    return (variables) => {
      if (!platform) platform = findPlatform();
      if (!platform) throw new Error("the client's Platform is not there (a Spotify update?)");
      return platform.getGraphQLLoader()(definition, variables);
    };
  };

  const names = (artists) => (artists || []).map((a) => a.name || (a.profile && a.profile.name) || "").filter(Boolean);
  const imageUrl = (url) => (url || "").replace(/^spotify:image:/, "https://i.scdn.co/image/");
  // The smallest picture: the lookup's image_url is the 64 px one.
  const smallest = (images) => {
    const list = (images || []).slice();
    if (!list.length) return "";
    const small = list.find((i) => i.label === "small") || list.slice().sort((a, b) => (a.width || a.height || 0) - (b.width || b.height || 0))[0];
    return imageUrl(small.url);
  };
  const releaseText = (release) => {
    if (!release || !release.isoString) return "";
    const date = release.isoString.slice(0, 10);
    if (release.precision === "YEAR") return date.slice(0, 4);
    if (release.precision === "MONTH") return date.slice(0, 7);
    return date;
  };

  // What playlists told about their tracks, for `lookup` (shared by every
  // caller of this page).
  const details = new Map();
  const remember = (item) => {
    if (!item || item.type !== "track" || !item.uri) return;
    details.set(item.uri, {
      uri: item.uri,
      type: "Song",
      name: item.name || "",
      parent: item.album ? { name: item.album.name || "", uri: item.album.uri || "" } : null,
      contributors: (item.artists || []).map((a) => ({ name: a.name || "", uri: a.uri || "" })),
      image_url: smallest(item.album && item.album.images),
      metadata: [
        { field: "duration", value: String((item.duration && item.duration.milliseconds) || 0) },
        ...(releaseText(item.release) ? [{ field: "release_date", value: releaseText(item.release) }] : []),
      ],
    });
  };

  const queueEntry = (item, provider) => ({
    uri: item.uri || "",
    uid: item.uid || "",
    name: item.name || "",
    artist: names(item.artists).join(", "),
    provider: item.provider === "queue" || provider === "queue" ? "queue" : "context",
  });

  const deviceJson = (d) => ({
    device_id: d.id,
    name: d.name || "",
    device_type: d.type || "",
    volume: Math.round((d.volume || 0) * 100),
    is_active: !!d.isActive,
    is_local: !!d.isLocal,
  });

  const libraryFilter = (label) => {
    const library = need("LibraryAPI");
    for (let id = 0; id < 16; id++) {
      if (library.getFilterLabel && library.getFilterLabel(String(id)) === label) return String(id);
    }
    throw new Error(`the library has no ${label} filter (a Spotify update?)`);
  };

  const calls = {
    // What the bridge can reach; `missing` lists what a Spotify update took away.
    probe() {
      platform = findPlatform();
      apis.clear();
      const wanted = {
        PlayerAPI: ["getState", "getQueue", "play", "pause", "resume", "skipToNext", "skipToPrevious", "seekTo", "setShuffle", "setRepeat", "addToQueue", "removeFromQueue"],
        PlaybackAPI: ["setVolume"],
        ConnectDevicesAPI: ["getDevices"],
        ConnectAPI: ["transferPlayback"],
        RootlistAPI: ["getContents"],
        PlaylistAPI: ["getContents", "getMetadata"],
        LibraryAPI: ["getContents", "getFilterLabel"],
        AuthorizationAPI: ["getState"],
      };
      const missing = [];
      if (!platform) missing.push("Platform");
      else {
        for (const [name, methods] of Object.entries(wanted)) {
          const found = api(name);
          if (!found) missing.push(name);
          else for (const method of methods) if (typeof found[method] !== "function") missing.push(`${name}.${method}`);
        }
        for (const query of ["decorateContextTracks", "searchModalResults"]) {
          try { gql(query); } catch (e) { missing.push(query); }
        }
      }
      const auth = platform && api("AuthorizationAPI");
      return {
        bridge: BRIDGE,
        client: navigator.userAgent.replace(/.*Spotify\/(\S+).*/, "$1"),
        missing,
        logged_in: !!(auth && auth.getState().isAuthorized),
      };
    },

    // The playlist tree in the sidebar's order, as `folder list` prints it.
    async library() {
      const contents = await need("RootlistAPI").getContents();
      const items = [];
      const walk = (list, depth) => {
        for (const item of list || []) {
          if (item.type === "folder") {
            items.push({ uri: item.uri || "", name: item.name || "", type: "folder", depth });
            walk(item.items, depth + 1);
          } else if (item.type === "playlist") {
            items.push({ uri: item.uri || "", name: item.name || "", type: "playlist", depth });
          }
        }
      };
      walk(contents.items, 1);
      return { items };
    },

    // Saved albums or followed artists, as `library list --type <kind>`.
    async saved({ kind }) {
      const library = need("LibraryAPI");
      const filter = libraryFilter(kind);
      const items = [];
      for (let offset = 0; ; ) {
        const page = await library.getContents({ filters: [filter], offset, limit: 200 });
        for (const item of page.items || []) {
          if (item.type === kind) items.push({ uri: item.uri, name: item.name || "", type: kind, artists: names(item.artists) });
        }
        offset += (page.items || []).length;
        if (!(page.items || []).length || offset >= (page.totalLength || 0)) return { items, total: items.length };
      }
    },

    // A whole playlist (or a station), as `playlist get` prints it.
    async playlist({ uri }) {
      const playlists = need("PlaylistAPI");
      let metadata = null;
      try { metadata = await playlists.getMetadata(uri); } catch (e) { /* a station has none */ }
      const tracks = [];
      let total = 0;
      for (let offset = 0; ; ) {
        const page = await playlists.getContents(uri, { offset, limit: 500 });
        total = page.totalLength || 0;
        for (const item of page.items || []) {
          remember(item);
          tracks.push({ type: item.type, uri: item.uri || "", name: item.name || "", artists: names(item.artists), album: (item.album && item.album.name) || "" });
        }
        offset += (page.items || []).length;
        if (!(page.items || []).length || offset >= total) break;
      }
      return {
        uri,
        name: (metadata && metadata.name) || "",
        description: (metadata && metadata.description) || "",
        owner: (metadata && metadata.owner && (metadata.owner.displayName || metadata.owner.username)) || "",
        track_count: (metadata && metadata.totalLength) || total,
        tracks,
      };
    },

    // Length, art and album, as `lookup` prints it: what playlists told,
    // then the client's own track query for the rest.
    async lookup({ uris }) {
      const missing = uris.filter((uri) => uri.startsWith("spotify:track:") && !details.has(uri));
      const decorate = missing.length ? gql("decorateContextTracks") : null;
      for (let at = 0; at < missing.length; at += 50) {
        const answer = await decorate({ uris: missing.slice(at, at + 50) });
        for (const track of (answer && answer.data && answer.data.tracks) || []) {
          if (!track || !track.uri) continue;
          const album = track.albumOfTrack || {};
          const artists = (track.artists && track.artists.items) || [];
          details.set(track.uri, {
            uri: track.uri,
            type: "Song",
            name: track.name || "",
            parent: { name: album.name || "", uri: album.uri || "" },
            contributors: artists.map((a) => ({ name: (a.profile && a.profile.name) || "", uri: a.uri || "" })),
            image_url: smallest(album.coverArt && album.coverArt.sources),
            metadata: [{ field: "duration", value: String((track.duration && track.duration.totalMilliseconds) || 0) }],
          });
        }
      }
      return { entities: uris.map((uri) => details.get(uri)).filter(Boolean) };
    },

    async search({ query }) {
      const answer = await gql("searchModalResults")({ searchTerm: query, offset: 0, limit: 30, numberOfTopResults: 30, includeAuthors: false });
      const found = { tracks: [], artists: [], albums: [], playlists: [] };
      const items = (answer && answer.data && answer.data.searchV2 && answer.data.searchV2.topResultsV2 && answer.data.searchV2.topResultsV2.itemsV2) || [];
      for (const { item } of items) {
        const data = item && item.data;
        if (!data || !data.uri) continue;
        switch (data.__typename) {
          case "Track":
            found.tracks.push({ uri: data.uri, name: data.name || "", artists: names(data.artists && data.artists.items) });
            break;
          case "Artist":
            found.artists.push({ uri: data.uri, name: (data.profile && data.profile.name) || "" });
            break;
          case "Album":
            found.albums.push({ uri: data.uri, name: data.name || "", artists: names(data.artists && data.artists.items) });
            break;
          case "Playlist":
            found.playlists.push({ uri: data.uri, name: data.name || "", author: (data.ownerV2 && data.ownerV2.data && data.ownerV2.data.name) || "" });
            break;
        }
      }
      return found;
    },

    // `now-playing` and `queue` in one answer.
    async playback() {
      const player = need("PlayerAPI");
      const state = player.getState();
      const queue = await player.getQueue();
      const item = state && state.item;
      const now = item && item.uri
        ? {
            description: `${item.name || ""} — ${names(item.artists).join(", ")}`,
            uri: item.uri,
            context_description: (state.context && state.context.metadata && (state.context.metadata.context_description || state.context.metadata.title)) || "",
            is_playing: !state.isPaused,
          }
        : null;
      const next = [
        ...((queue && queue.queued) || []).map((i) => queueEntry(i, "queue")),
        ...((queue && queue.nextUp) || []).map((i) => queueEntry(i, "context")),
      ];
      // Each half as its own JSON text: the Rust side reads them with the
      // CLI's `now-playing` and `queue` parsers.
      return {
        now: JSON.stringify({ currently_playing: now }),
        queue: JSON.stringify({ currently_playing: item && item.uri ? queueEntry(item, "context") : null, next_tracks: next }),
      };
    },

    devices() {
      const devices = need("ConnectDevicesAPI").getDevices() || [];
      return { devices: devices.map(deviceJson) };
    },

    async transport({ command, value }) {
      const player = need("PlayerAPI");
      switch (command) {
        case "play": await player.play({ uri: value }, {}, {}); break;
        case "resume": await player.resume(); break;
        case "pause": await player.pause(); break;
        case "next": await player.skipToNext(); break;
        case "previous": await player.skipToPrevious(); break;
        case "seek": await player.seekTo(Number(value)); break;
        case "shuffle": await player.setShuffle(!!value); break;
        // 0 off, 1 context, 2 track.
        case "repeat": await player.setRepeat(Number(value)); break;
        case "volume": await need("PlaybackAPI").setVolume(Number(value)); break;
        // Connect logs every transfer under an interaction id; it has to be there.
        case "transfer": await need("ConnectAPI").transferPlayback(value, { interactionId: crypto.randomUUID() }); break;
        case "enqueue": await player.addToQueue([{ uri: value }]); break;
        case "dequeue": {
          const queue = await player.getQueue();
          const next = [...((queue && queue.queued) || []), ...((queue && queue.nextUp) || [])];
          const entry = next[Number(value)];
          if (!entry) throw new Error(`nothing at position ${value} of the queue`);
          await player.removeFromQueue([{ uri: entry.uri, uid: entry.uid }]);
          break;
        }
        default: throw new Error(`unknown command ${command}`);
      }
      return {};
    },
  };

  window.__makepadAmp = {
    tag,
    bridge: BRIDGE,
    async call(name, args) {
      try {
        const call = calls[name];
        if (!call) return `err:the bridge has no ${name}`;
        return "ok:" + JSON.stringify(await call(JSON.parse(args || "{}")));
      } catch (error) {
        return "err:" + ((error && error.message) || String(error));
      }
    },
  };
  return tag;
})
