# Kloudlite SDK for Java

```xml
<dependency>
  <groupId>io.kloudlite</groupId>
  <artifactId>kloudlite-sdk</artifactId>
  <version>0.5.0</version>
</dependency>
```

Requires Java 11 or newer. No dependencies.

A snapshot push is a storage commit; a branch names one and moves like a git branch:

```java
kl.workspaces().takeSnapshot(new Kloudlite.TakeWorkspaceSnapshot().workspace("api").branch("wip-auth").labels("auth"));
kl.volumes().list(new Kloudlite.ListVolumes().label("auth"));
kl.volumes().describe(new Kloudlite.DescribeVolume().owner("acme").name("api-home").description("API dev home"));
kl.volumes().setBranch(new Kloudlite.SetBranch().owner("acme").name("api-home").branch("base").version(4));
kl.volumes().deleteBranch("acme", "api-home", "wip-auth");
```
