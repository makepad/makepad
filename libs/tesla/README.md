# makepad-tesla

Event-driven Tesla Fleet API client on the makepad network layer. Purpose: poll
battery / charge state of your own car so the GPS app can do charger-aware routing.

## One-time setup for your own car

Tesla killed the old unofficial owner API; the official Fleet API needs a (free)
developer app registration, even for your own single car. Steps:

### 1. Developer app

1. Tesla account needs a verified email + multi-factor auth enabled.
2. Go to <https://developer.tesla.com> → request app access. Fill in a name and
   "personal use: vehicle data for private navigation app" as purpose.
3. OAuth Grant Type: **Authorization Code and Machine-to-Machine**.
4. Allowed Origin: a domain you control (e.g. `https://n4.io/`).
5. Allowed Redirect URI: something you can read the address bar on, e.g.
   `https://n4.io/tesla-callback` — the page does not need to exist, you just
   copy the `?code=` out of the URL after login.
6. Scopes: enable at least `vehicle_device_data` (Vehicle Information).
   `vehicle_location` if the app should also read the car's own GPS position.
7. You get a **client_id** and **client_secret**.

### 2. Host the public key + register the partner account

Fleet API refuses all calls until the app's domain is registered:

```bash
# generate an EC key pair (the private key is only needed for vehicle *commands*,
# but the public half must be hosted for registration)
openssl ecparam -name prime256v1 -genkey -noout -out tesla_private.pem
openssl ec -in tesla_private.pem -pubout -out tesla_public.pem
```

Host `tesla_public.pem` at:

```
https://<your-domain>/.well-known/appspecific/com.tesla.3p.public-key.pem
```

Then register (one time, with a machine-to-machine "partner token"):

```bash
# partner token
curl -s https://fleet-auth.prd.vn.cloud.tesla.com/oauth2/v3/token \
  -d grant_type=client_credentials \
  -d client_id=$CLIENT_ID -d client_secret=$CLIENT_SECRET \
  -d scope='openid vehicle_device_data' \
  -d audience=https://fleet-api.prd.eu.vn.cloud.tesla.com

# register the domain (use the access_token from above)
curl -s https://fleet-api.prd.eu.vn.cloud.tesla.com/api/1/partner_accounts \
  -H "Authorization: Bearer $PARTNER_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"domain":"<your-domain>"}'
```

(Use the `na` host instead of `eu` if the car is North-American.)

### 3. Log in as yourself, get the refresh token

Open this in a browser (fill in client_id + redirect_uri), log in with the
Tesla account that owns the car:

```
https://auth.tesla.com/oauth2/v3/authorize?response_type=code&client_id=<CLIENT_ID>&redirect_uri=<REDIRECT_URI>&scope=openid+offline_access+vehicle_device_data+vehicle_location&state=makepad
```

Copy the `code=` value from the redirect URL, then exchange it:

```bash
curl -s https://fleet-auth.prd.vn.cloud.tesla.com/oauth2/v3/token \
  -d grant_type=authorization_code \
  -d client_id=$CLIENT_ID -d client_secret=$CLIENT_SECRET \
  -d code=$CODE \
  -d redirect_uri=$REDIRECT_URI \
  -d audience=https://fleet-api.prd.eu.vn.cloud.tesla.com
```

The response contains `access_token` (valid 8h) and `refresh_token`.

### 4. Credentials file

Put the result in `tesla_credentials.json` in the repo root (it is untracked,
same pattern as `GOOGLE_API_KEY`):

```json
{
    "client_id": "…",
    "refresh_token": "…",
    "region": "eu"
}
```

(Optionally add `"client_secret": "…"` — only needed if Tesla rejects the
token refresh without it; the library sends it when present.)

That's all the library needs. It refreshes the access token itself and
**rewrites this file** on every refresh, because Tesla rotates refresh tokens —
don't hand-edit it afterwards, and don't reuse the same refresh token elsewhere.

## Costs / rate limits

Personal accounts get a small monthly usage credit (about $10). A
`vehicle_data` poll costs a fraction of a cent; polling every few minutes while
driving stays comfortably inside the free credit. Wake-ups are the expensive
call — the library never wakes the car unless explicitly asked to.

## Library usage

See `src/lib.rs` docs. Everything is event-driven on the makepad network layer:
you call `request_*` methods with a `&mut Cx`, and route
`Event::NetworkResponses` through `handle_event`, which yields typed
`TeslaClientAction`s.
