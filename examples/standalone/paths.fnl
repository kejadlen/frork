;; A local module: inlined into the binary at build time.
(local {: utils} (require :frork))

(local root (utils.expand-path "$FRORK_DEMO_ROOT"))

{: root
 :path (fn [...] (table.concat [root ...] "/"))}
