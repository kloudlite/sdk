# Kloudlite SDK for Java

```xml
<dependency>
  <groupId>io.kloudlite</groupId>
  <artifactId>kloudlite-sdk</artifactId>
  <version>0.5.0</version>
</dependency>
```

Requires Java 11 or newer. No dependencies.

A snapshot push is a storage commit; a tag names one and moves only when asked:

```java
kl.workspaces().takeSnapshot(new Kloudlite.TakeWorkspaceSnapshot().workspace("api").tag("wip-auth").labels("auth"));
kl.volumes().list(new Kloudlite.ListVolumes().label("auth"));
kl.volumes().describe(new Kloudlite.DescribeVolume().owner("acme").name("api-home").description("API dev home"));
kl.volumes().setTag(new Kloudlite.SetTag().owner("acme").name("api-home").tag("base").version(4));
kl.volumes().deleteTag("acme", "api-home", "wip-auth");
```
