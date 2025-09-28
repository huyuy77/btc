# Project BTC

BitTorrent Tracker Cache ("BTC") is a generic peer list cache service. It allows users to communicate with other peers without reporting real statistics to trackers.

If you use public trackers or lack familiarity with the mechanism of private trackers, this may sound weird. But the only way PT knows about your upload/download is through the data reported by your client when requesting for a peer list ("announce"). Therefore, if we manually construct and send a request to the tracker mocking the start of a session, and store the peer list received, then we could just use this list to connect to peers directly.

Using this project, the torrent session **does not** count towards your ratio at private tracker sites. You cannot build fake uploads using this service, but you can download for free.

## Deployment

Under most circumstances, you could use the public instance hosted at https://tracker.submy.org. That being said, you may encounter some of these problems:

* Your tracker enforces IP whitelist, or blocks IP from certain regions or not meeting some criteria
* You don't trust the public instance, thinking it would steal your passkeys
* The public instance is overloaded or under attack and therefore could not serve your requests

You could deploy your own instance of your own free will. There are three environment variables that helps you customize the deployment:

* **BASE_URL:** The host address of this service. This is how BitTorrent clients connect to your service, and used for replacing the tracker URL in torrents. Example: `BASE_URL=https://localhost:3000`
* **PROXY:** The traffic of all requests to the origin trackers will pass through this proxy if set. Example: `PROXY=http://localhost:8080`
* **CACHE_ROOT:** By default this project uses `$XDG_CACHE_HOME/btc` as its cache directory. You could set it to another location if your home directory does not have sufficient space. Example: `CACHE_ROOT=/mnt/another_drive/.cache`

You may also want to modify the upload URL in [www/static/index.html](./www/static/index.html). Its host should be identical to `BASE_URL`.

## Advanced Usage

The web interface only modifies the torrent you upload so that it uses our tracker instead of the built-in ones. In fact, you could manually do it through torrent file editors or BitTorrent clients that supports editing tracker URLs (e.g. qBittorrent). The URL format of this project is:

```
https://tracker.submy.org/announce?tracker_url=<redacted>&ttl=28800
```

The `tracker_url` is the percent-encoded form of the origin tracker URL, and `ttl` is the time duration in seconds that the cache should live at a minimum. If the torrent is relatively new, you could set `ttl` to smaller values to update the cache more frequently. For very old torrents, the seeders are likely to be fixed, so you set `ttl` longer.

## Credits

This project is a web service implementation of the idea from the insightful repository [lyc8503/PTHackPoC](https://github.com/lyc8503/PTHackPoC). A huge thanks to him for spotting and pointing out the vulnerability of private trackers.
