# T-005 apply and survive

macOS: 27.2 (26B123)

## Signature of the built binary
```
Identifier=sandbox_custom-6c913592a97fe9a7
CodeDirectory v=20400 size=3960 flags=0x20002(adhoc,linker-signed) hashes=120+0 location=embedded
Signature=adhoc
TeamIdentifier=not set
```

## Confinement from the unsigned build
```
inside-ok
touch: /private/tmp/outside.txt: Operation not permitted
outside-denied
```
