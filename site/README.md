# SedravaSQL official site
Canonical URL: https://sql.virtreal.ru/

Static Russian landing and guide; source lives in SedravaSQL GitHub repo.
No FastPanel token, dynamic server key, SQL service or backend is published.

## Checks and release

    python3 -B scripts/verify_official_site.py
    python3 -B scripts/deploy_official_site.py          # dry run
    python3 -B scripts/deploy_official_site.py --deploy # explicit publish

Deploy only from the website owner account on the existing FastPanel VDS.
No changes to Nginx, certificates, other sites or GitHub Actions.
Existing public files are backed up outside the webroot before publishing.
The status.json endpoint is descriptive, NOT a live engine status or health API.
