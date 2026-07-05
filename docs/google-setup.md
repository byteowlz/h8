# Google Workspace / Gmail account setup

Step-by-step onboarding for a Google account (`provider = "google"`) plus a
manual smoke-test checklist. See `docs/design/multi-provider.md` section 4 for
the underlying API mapping decisions, and the README's "Google Workspace
setup" section for the short version.

## 1. Create (or pick) a GCP project

1. Go to the [Google Cloud Console](https://console.cloud.google.com) and
   create a new project, or select an existing one you control.
2. Under **APIs & Services -> Library**, enable each of:
   - **Gmail API**
   - **Google Calendar API**
   - **People API**

   All three are required even for a personal single-user setup -- h8 uses
   Gmail for mail, Calendar for calendar, and People for contacts.

## 2. Configure the OAuth consent screen

1. **APIs & Services -> OAuth consent screen**. Choose **External** as the
   user type (Internal is only offered for Google Workspace orgs and is not
   needed for a personal Gmail account).
2. Fill in the required app fields (name, support email, developer contact).
   You do not need to add scopes here manually; the client requests them at
   login time.
3. Add your own Google account under **Test users** while you are iterating.
4. **Publish the app to "In Production"** once you're done configuring it.

   **This step is not optional.** While the consent screen stays in
   **Testing**, Google issues refresh tokens that **expire after 7 days** --
   the background token-refresh loop will work fine for a week and then start
   failing with `LoginRequired`, which looks like a bug but is just an
   expired refresh token. Publishing to Production removes the 7-day limit.
   Google will show an "unverified app" warning to anyone who logs in
   (including you); for personal/single-user use this warning is
   click-through-able ("Advanced -> Go to \<app name\> (unsafe)") and does not
   block the OAuth flow. Full verification review is not required unless you
   plan to distribute the app to other users.

## 3. Create an OAuth client

1. **APIs & Services -> Credentials -> Create Credentials -> OAuth client ID**.
2. Application type: **Desktop app** (this is the type h8 expects; it allows
   both the loopback browser flow and the headless URL-paste flow with the
   same client).
3. Copy the **Client ID** and **Client secret**. For an installed/desktop
   app, the "secret" is not actually confidential (Google's own docs note
   this) -- it is still worth keeping your config file readable only by you.

## 4. Add the account to config.toml

```toml
[accounts.personal]
email = "you@gmail.com"
provider = "google"
client_id = "xxxx.apps.googleusercontent.com"
client_secret = "xxxx"
```

Pick any alias (`personal` here); reference it with `--account personal` or
make it the default with a top-level `account = "personal"`.

## 5. Sign in

```bash
h8 auth login personal
```

- **On a machine with a browser**: this starts a local loopback listener and
  opens (or prints a link to) an authorization page. Approve access; the CLI
  detects completion automatically.
- **On a headless host** (no browser reachable, e.g. SSH session): h8 prints
  an authorization URL instead of trying to open a browser. Steps:
  1. Copy the printed URL and open it in a browser on any machine (your
     laptop, phone, etc.).
  2. Log in with the Google account and approve the requested scopes. Click
     through the "unverified app" warning if the app is still in Testing (or
     even after publishing, if you haven't gone through verification).
  3. After approving, Google redirects to a `http://localhost/...` URL. That
     page will fail to load (nothing is listening on your browsing machine)
     -- that's expected. Copy the full URL from the address bar.
  4. Paste the full URL back into the `h8 auth login` prompt on the headless
     host to complete the exchange.

Verify with:

```bash
h8 auth status personal
```

which should show a logged-in state and a token expiry.

## 6. Manual smoke checklist

Google's own API sandboxes and rate limits vary account to account, so after
setup it's worth manually exercising the surface once against the real
account. Use `--account personal` (or whatever alias you chose) on every
command below.

Supported (should succeed):

```bash
h8 mail list --account personal                       # mail:read
h8 mail send --account personal --to you@gmail.com --subject "h8 smoke test" --body "hello"
h8 mail list --account personal                        # confirm it landed in inbox/sent
h8 mail read <id> --account personal                    # mail:read
h8 mail search "subject:smoke" --account personal       # mail:read

h8 cal add --account personal "tomorrow 10am-10:30am h8 smoke test"   # calendar:write
h8 cal show tomorrow --account personal                 # calendar:read
h8 cal delete <id> --account personal                    # calendar:write

h8 contacts list --account personal                      # contacts:read

h8 oof status --account personal                         # oof:read
```

Expected to fail with HTTP 501 (`missing_capability`) -- Google does not
advertise these, so this is correct, not a bug:

```bash
h8 addr search "someone" --account personal              # CAP_GAL not supported
h8 resource free cars today --account personal            # CAP_RESOURCES not supported
h8 rules list --account personal                          # CAP_RULES not supported
```

You can also check the full capability set directly instead of triggering
each 501 by hand, via the (unauthenticated) HTTP endpoint:

```bash
curl "http://127.0.0.1:8787/capabilities?account=personal"
```

which should list `mail`, `mail:send`, `mail:folders`, `calendar`,
`calendar:meetings`, `contacts`, `freebusy:self`, `freebusy:others`, `oof` and
omit `gal`, `resources`, `rules`, `mail:scheduled_send`, `bookings`.
