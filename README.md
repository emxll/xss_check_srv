# xss_check_srv
Simple xss challenge check polling service

Client 1 makes req to `/poll-notified?token=abcd`, which blocks.
Client 2 makes req to `/notify?token=abcd&secret=shhh`
Req 1 unblocks into
```
{"secret":"shhh"}
```

Might have to increase proxy_read_timeout if behind nginx.