# SedravaSQL official site
Canonical URL: https://sql.virtreal.ru/

Static Russian landing and guide; source lives in SedravaSQL GitHub repo.
No FastPanel token, dynamic server key, SQL service or backend is published.

## Checks and release

    python3 -B scripts/verify_official_site.py
    python3 -B scripts/deploy_official_site.py          # dry run
    python3 -B scripts/deploy_official_site.py --deploy # explicit publish

Deploy through Gateway with authorized sudo on the existing FastPanel VDS. Actual
document root was verified via read-only FASTPANEL API site ID 35:
/var/www/virtreal_ru_usr27/data/www/sql.virtreal.ru (owned by virtreal_ru_usr27).
No changes to Nginx, certificates, other sites or GitHub Actions.
Existing public files are backed up outside the webroot before publishing.
The status.json endpoint is descriptive, NOT a live engine status or health API.
