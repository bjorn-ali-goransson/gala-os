import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;

/** gala's java_hello scenario: run as `java Hello.java` (compiled in memory). */
public class Hello {
    public static void main(String[] args) throws Exception {
        System.out.println("Hello from Java on gala");
        for (String p : new String[] {"os.name", "os.version", "os.arch", "user.name", "user.home", "java.home"}) {
            System.out.println(p + "=" + System.getProperty(p));
        }
        System.out.println("processors=" + Runtime.getRuntime().availableProcessors());

        Map<String, Integer> counts = new TreeMap<>();
        for (String w : "the quick brown fox jumps over the lazy dog the end".split(" ")) {
            counts.merge(w, 1, Integer::sum);
        }
        System.out.println("counts=" + counts);

        ExecutorService pool = Executors.newFixedThreadPool(4);
        List<Future<Long>> parts = new ArrayList<>();
        for (int t = 0; t < 4; t++) {
            final int k = t;
            parts.add(pool.submit(() -> {
                long s = 0;
                for (long i = k; i < 1_000_000; i += 4) {
                    s += i;
                }
                return s;
            }));
        }
        long sum = 0;
        for (Future<Long> f : parts) {
            sum += f.get();
        }
        pool.shutdown();
        System.out.println("sum=" + sum);
        System.out.println(String.format("pi=%.5f", Math.PI));
    }
}
