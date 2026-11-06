# Git's https remotes on Schannel: MinGit (PREBUILT mingit), unmodified,
# against schannel_test's Nginx (user/schannel_test/nginx.conf), which
# serves repo.git (a bare repository with one commit, `git
# update-server-info` run, the files Git's "dumb" HTTP protocol reads) as
# C:\nginx\html\repo.git. Run by busybox sh in the `git_https` scenario.
# Git for Windows' libcurl validates through the machine's root store
# (gala: System32\gala\roots, where the scenario puts the test CA);
# http.schannelCheckRevoke=false as curl's --ssl-no-revoke (the test
# certificates name no CRL distribution point). Each line names its step
# and gives git's exit code.
git=C:/git/cmd/git.exe
set -- -c http.sslBackend=schannel -c http.schannelCheckRevoke=false
url=https://localhost:8443/repo.git

# Nginx is started with the boot: wait for it to answer.
i=0
until $git "$@" ls-remote $url >/dev/null 2>&1 || [ $i -ge 30 ]; do
    sleep 1
    i=$((i + 1))
done
refs=$($git "$@" ls-remote $url)
echo "ls-remote: exit $? $refs"

$git "$@" clone -q $url C:/Users/gala/clone
echo "clone: exit $?"
echo "file: $(cat C:/Users/gala/clone/hello.txt)"
echo "log: $($git -C C:/Users/gala/clone --no-pager log --format=%s)"

$git "$@" ls-remote https://localhost:8444/repo.git
echo "untrusted: exit $?"

echo "git.sh: done"
