## 1. Increment a version

```
export PKG_VER=YOUR_VERSION_HERE
./increment_version.sh $PKG_VER
git commit -m "Incremented version to $PKG_VER" package.json src/ios-deploy/version.h
```

## 2. Tag a version

```
git tag $PKG_VER
```

## 3. Push version and tag

```
git push origin master
git push origin $PKG_VER
```

## 4. Publish to npm

```
npm publish
```

## 5. Publish to Homebrew

```
brew bump-formula-pr --url="https://github.com/ios-control/ios-deploy/archive/${PKG_VER}.tar.gz" ios-deploy
```
