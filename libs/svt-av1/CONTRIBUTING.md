# How to Contribute

We welcome community contributions to the SVT-AV1 Encoder. Thank you for your time! By contributing to the project, you agree to the license, patent and copyright terms in the AOM License and Patent License  and to the release of your contribution under these terms. See [LICENSE](LICENSE.md) and [PATENTS](PATENTS.md) for details.

## Contributor agreement

You will be required to execute the appropriate [contributor agreement](http://aomedia.org/license/) to ensure that the AOMedia Project has the right to distribute your changes.

## Contribution process

- Follow the [coding guidelines](STYLE.md)
- Validate that your changes do not break a build
  - either locally or through travis-ci and github actions. Preferably all of them.
- Perform smoke tests and ensure they pass
- Submit a pull request for review to the maintainer

## Pull request process

- Authors should use a valid email account when committing.
- Make clear and concise commits (1 commit per 1 feature or issue)
- Authors are responsible for breaking down the PR into sensible commits (with proper commit messages)
- Avoid using force push when addressing comments and review items.
- Maintainers shall use 'rebase and merge' to make sure all commits can apply cleanly onto the master branch
- Maintainers shall only use 'squash and merge' with the permission of the authors.
