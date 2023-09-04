# criterion-single-page-html

> Experimental shit code, yada yada

## the pitch

Using [concourse ci](https://concourse-ci.org) and referencing artifacts from github issues comments
in an automated fashion, there is a need to quickly serve `criterion` bench runs against a baseline
as part of a changeset, i.e. a pull request.
Now, one could host and maintain a webserver with credentials to host the tree of files and folders
that criterion generates, but that is cumbersome and infra shall be free of maintenence (for me) and
be replacable. Here comes `s3`.

The _super correct_ path would be to implement this as criterion render output, but this has multiple
reasons to not do so, mostly due to alternative use cases across harness outputs.

## what does it do?

It pulls in all files linked to `<a href..>` with relative paths starting from `--root` and integrates them
into sections with unique ids, derived from the file content. There is special handling for linked `.svg` files.

The `<link src=..` will be converted to inline data urls.

Boring, ey?

## caveats

* If you have a few 100 runs and svgs inlined, it becomes _really_ slow. Remember, `<section style="display=none"..`s only _hide_
the content yet still have to render it. Using inline svg files doesn't improve the situation much. Prepare for
slow and unhappy browser tabs.
* Currently svgs are not traversed besides the `<title`, which can lead to issues in case of _local_ fonts.
* Any links pointing to `http://` or `https://` urls will not be touched, only _relative_ urls will be transformed.

Again, it's focused on dealing with criterion output primarily.

If you found this useful, perfect!
