;; A macro module: expanded at build time, absent from the binary.
(fn ok-all [kind ...]
  `(do ,(unpack (icollect [_ path (ipairs [...])] `(ok ,kind ,path)))))

{: ok-all}
