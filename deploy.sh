set -e
cd "$(dirname "$0")"
. ./deploy.env
rsync -az --delete --exclude target --exclude .git --exclude out --exclude deploy.env --exclude secrets.env ./ "$DEPLOY_HOST:embedgen/"
# over stdin, so the token never appears in a command line
printf 'JBCL_TOKEN=%s\n' "${JBCL_TOKEN:-}" | ssh "$DEPLOY_HOST" 'umask 077; cat > embedgen/secrets.env'
ssh "$DEPLOY_HOST" bash -se <<'R'
set -e
cd ~/embedgen && docker build -q -t embedgen . 2>&1 | tail -5
docker rm -f embedgen 2>/dev/null || true
docker run -d --name embedgen --restart unless-stopped -p 127.0.0.1:8140:8080 --env-file ~/embedgen/secrets.env \
  -v ~/embedgen/embeds.json:/app/embeds.json:ro -v ~/embedgen/out:/app/out:ro embedgen
sleep 3; docker logs embedgen
R
