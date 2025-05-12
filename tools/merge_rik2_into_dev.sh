git stash
git branch -D dev
git fetch
git checkout dev
git checkout rik2
git rebase dev 
git checkout dev
git merge rik2 --ff-only 
git push 
git checkout rik2 
git stash apply
