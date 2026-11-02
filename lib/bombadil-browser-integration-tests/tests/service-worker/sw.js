// Cache first: script.js is fetched by the worker at install and served from
// its cache from then on.
self.addEventListener("install", (event) => {
    event.waitUntil(
        caches
            .open("scripts")
            .then((cache) => cache.add("./script.js"))
            .then(() => self.skipWaiting()),
    );
});

self.addEventListener("fetch", (event) => {
    event.respondWith(
        caches
            .match(event.request)
            .then((cached) => cached ?? fetch(event.request)),
    );
});
