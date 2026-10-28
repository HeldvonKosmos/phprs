package bench;

import org.springframework.context.annotation.Configuration;
import org.springframework.web.servlet.config.annotation.InterceptorRegistry;
import org.springframework.web.servlet.config.annotation.WebMvcConfigurer;

/** The token check on the notes and the logic workloads, as in the Laravel app. */
@Configuration
public class WebConfig implements WebMvcConfigurer {
    private final BenchToken benchToken;

    public WebConfig(BenchToken benchToken) {
        this.benchToken = benchToken;
    }

    @Override
    public void addInterceptors(InterceptorRegistry registry) {
        registry.addInterceptor(benchToken).addPathPatterns("/api/bench/sql/notes", "/api/bench/sql/notes/**", "/api/bench/logic/**");
    }
}
