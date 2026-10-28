plugins {
	java
	id("org.springframework.boot") version "4.1.2"
	id("io.spring.dependency-management") version "1.1.7"
}

group = "bench"
version = "0.0.1-SNAPSHOT"

java {
	toolchain {
		languageVersion = JavaLanguageVersion.of(25)
	}
}

repositories {
	mavenCentral()
}

// BENCH3: every resolved version is locked (gradle.lockfile; refresh with
// ./gradlew dependencies --write-locks)
dependencyLocking {
	lockAllConfigurations()
}

dependencies {
	implementation("org.springframework.boot:spring-boot-starter-jdbc")
	implementation("org.springframework.boot:spring-boot-starter-validation")
	implementation("org.springframework.boot:spring-boot-starter-webmvc")
	// the driver version made for SQLite 3.53.4; its JNI part is rebuilt
	// against the benchmark's libsqlite3 (scripts/build_sqlite_jni.sh)
	implementation("org.xerial:sqlite-jdbc:3.53.4.0")
	// BCryptPasswordEncoder only (no Spring Security filter chain)
	implementation("org.springframework.security:spring-security-crypto")
	// development mode only (hot reload, bench/frameworks/hotreload.py); not in the boot jar
	developmentOnly("org.springframework.boot:spring-boot-devtools")
}
